# R-27 修正ブリーフ (host lease の移譲)

実装は Codex、検収は ClaudeCode。**着手前に
[docs/detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) を読むこと。**
本件はリワークのステージ外からの構造的修正で、§2 の適用範囲どおり ClaudeCode と Codex の
双方が「症状パッチではなく構造的修正である」ことに合意済み (合意の経緯は
[README.md](README.md) §10.6)。完了後は同プラン §11 に触れた範囲を記録する。

## 直す不具合

`Detached(H) -> Fullscreen` の retire 待ちに `Detached` を要求すると、`NativeRetired` の
**同じ effect batch** が `CloseDetachedSession` / `DestroyHost { hwnd: H }` を出しながら、
後継を `ReadyToPrepare { host_hwnd: H }` にする。表示は Fullscreen のまま、窓は閉じ、
設定とトーストだけ Detached ON で残り得る。

再現テストは
[src/app/presentation_transition.rs](../../src/app/presentation_transition.rs) の
`a_successor_does_not_reuse_the_host_the_same_batch_destroys` (現在 `#[ignore]`)。
**このテストの期待値 (`AwaitingHost` へ倒す) は暫定で、下の設計に合わせて書き換えてよい。**
`#[ignore]` は必ず外すこと。

## 合意した設計 (案 A′ = lease の移譲 + retire 後の再検証)

Codex の調査で確定した事実:

- `DestroyHost` は H に `DestroyWindow` していない。現在の `detached_viewer_window_id` から
  ViewportId を再解決して `ViewportCommand::Close` を送るだけで、effect の `hwnd` はログにしか
  使われない (`native_video.rs:1627`)。
- `CloseDetachedSession` は active session / binding / `DetachedWindowRuntime` を除去する。
- `detached_viewer_window_id` とその場の OS HWND は壊れない。
- native presenter は H 自身ではなく H 配下の `WS_CHILD`。retire が壊すのは child と
  その render core / DComp / GPU 資源だけで、**親 H には再利用不能な状態が残らない**
  (SetParent / style / subclass / DComp target はいずれも child 側)。
- 後継の `Switch` も retire と同じ native pump FIFO に後から入るので、旧 child の Destroy を
  追い越さない。

したがって「同じ egui parent H を後継 presenter の親として再利用する」は native contract 上
成立する。**単純な案 B (Destroy してから `AwaitingHost`) は採らない** — 旧 viewport の close 完了
→ 新 window identity の確保 → 新 HWND 登録という因果経路が無く、結果が lifecycle ordering
依存になる (同じ ViewportId を render し続けるので、まだ生きている H を再び `HostReady` に
するか、`ViewportEvent::Close` を利用者の close と解釈して後継ごと閉じるかのどちらにもなる)。

**単純な案 A (同一 H なら Close/Destroy を省くだけ) も採らない。** 失敗・再置換・terminal close
で session が残る。

採る形:

```text
RetiringCommitted(old session lease, successor)
  -> NativeRetired
  -> old lease を successor へ transfer、Close/Destroy は出さない
  -> AwaitingHost(transferred session lease)
  -> 現時点の HostReady({window_id, host incarnation, hwnd})
  -> PrepareNative(exact host claim)
```

`AwaitingHost` を経由するのは作り直しのためではなく、**要求時に観測した H を非同期 retire
境界の向こうまで durable な事実として扱わないため**の再検証。H がまだ有効なら即座に H が
返り、その間に自然な host-loss で別の窓 J になっていれば J を使う。

## 併せて閉じる identity gap

現在は producer → reducer → effect → executor が同じ host を指していない:

- `ready_host_hwnd` が raw HWND だけを持つ
- `PrepareNative { host_hwnd }` の executor はその値を**使わず**、current global host を
  再解決する (`native_video.rs:1469`, `app.rs:42241`)

`{window_id, host incarnation, hwnd}` の claim で 4 段を結ぶ。runtime が Closing / lease が違う /
incarnation が変わった場合だけ unready とし、`HostUnavailable -> AwaitingHost` へ戻す。

**`ready_host_hwnd` の述語を「retire 中の H は ready ではない」に変える修正は入れない。**
移譲が成立した時点で H は破棄予定ではなく successor 所有の live host なので、意味が変わる。

## 触ってよい範囲

- [src/app/presentation_transition.rs](../../src/app/presentation_transition.rs) —
  host/session lease 型、`RetiringCommitted`、`finish_retired_then_start`、replacement /
  abort / `NativeFailed` / terminal close
- [src/app/native_video.rs](../../src/app/native_video.rs) — ready producer、`AwaitingHost` poll、
  `PrepareNative` / `CloseDetachedSession` / `DestroyHost` の exact identity 実行
- [src/app.rs](../../src/app.rs) — window ID 指定の host claim 取得・検証、exact session close
- [src/app/detached_window_manager.rs](../../src/app/detached_window_manager.rs) —
  HWND 再利用まで防ぐなら per-runtime host incarnation
- [src/app/tests.rs](../../src/app/tests.rs)

`src/video/` の production 変更は不要。

**他のファイルは触らないこと。** 同じ作業ツリーで ClaudeCode が並行して
`src/gpu_lanczos.rs` / `src/displayed_image_transform.rs` / `src/ui_fullscreen.rs` を編集している。
コミットは `git commit -- <自分のパス>` の pathspec commit で行い、`git add -A` は使わない。

## 完了条件

1. `a_successor_does_not_reuse_the_host_the_same_batch_destroys` の `#[ignore]` を外し、緑にする。
2. 次のテストを追加する:
   - alias lease で Close/Destroy が出ないこと
   - 要求時 H / retire 後 J の再取得ケース
   - non-alias lease は旧 host だけ close し、新 host だけに prepare すること
   - transfer 後の `NativeFailed` / 再置換 / abort / terminal close で **一度だけ**解放されること
   - `DestroyHost(old)` / `CloseDetachedSession(old)` が current sibling を閉じないこと
   - `PrepareNative(claim H)` が current global J へ黙って逸れないこと
3. 既存の Detached→Fullscreen / Detached→Detached KeepLive のテストは無変更で緑のまま。
   憲法規則 8 のとおり、既存 detached テスト 104 本を削除・弱体化しない。
4. `cargo test -p mimageviewer --lib` が緑。`cargo fmt` 済み。
5. 憲法規則 3 (新しい App-level bool / Option を足さない) と規則 5 (時間窓・retry・
   settle で吸収しない) に反していないことを完了報告で明示する。
6. コミットメッセージに `(detached-rework R-27)` を含める。

## 報告してほしいこと

- 追加した型と、それが規則 3 に触れない理由
- 既存テストを 1 本でも書き換えたなら、その理由 (規則 8)
- 実機 smoke が要る操作 (F12 連打の具体的な手順)
