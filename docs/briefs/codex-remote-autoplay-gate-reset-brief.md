# mIV Remote: 自動再生の待ちが、誰かに解除されている

worktree: `C:\home\mimageviewer-web` / branch `web-remote` / 起点 `c4e341f6`

## 0. 立場

**本体が正本。独自の規則を発明しない。**
以下は私が記録とソースから読み取った内容だが、**実際と違っていたら実際の方を報告してほしい。**
私の要約に合わせて実装しないこと。この段取りで既に私の誤りが 18 回訂正されている。

稼働中の本体 / remote-web は操作しない。`build-dev.ps1`・コミットも実行しない。

## 1. 観測

`87897b24` の自動再生対応を実機で確認したところ、**直っていなかった。**

利用者の報告:

> 再読込後、画面は真っ黒のままで**特にタップで再生の案内などは出ません**。
> その状態でタップすると、**動画の再生が停止しました**と出て動画は始まりませんでした。

**版の確認は済んでいる。** 今回入れた版テレメトリで、
`running_asset_token` = `served_asset_token` = `4c0bb8284407d9c0` を確認した。
**新しい版で動いていた。** 古い版で試した可能性は排除できる。

## 2. 端末側の連番で並べた時系列 (バッチ送信で崩れるのでサーバ受信時刻ではない)

ページ再読込で連番が 1 に戻った時点を起点にしている。

```
+47.02  seq  3,4   remote_session acquire / app_version   版は一致
+49.23  seq  5,6   play_rejected   att=1 rej=1  rs=1  paused=true
+49.25  seq  7,8   play_rejected   att=2 rej=2  rs=3  paused=true   ← 拒否は NotAllowedError
+58.48  seq  9     media_stalled                rs=4  paused=true
+61.48  seq 11,12  playback_waiting             waiting_trigger="stalled"  elapsed_ms=3002
+73.48  seq 14,15  playback_stalled             waiting_trigger="stalled"  elapsed_ms=15003
                                                → 「動画の再生が停止しました」
```

以後、位置 0 / paused=true / rs=4 / 先読み 30 秒のまま 10 秒ごとのサンプルが続く。

## 3. ここまでで確定していること

- **+49.25 の時点で gate は立っている。** 拒否ハンドラは
  `recordPlaybackIssue` → `captureVideoHealth` → `playbackGate = "user_activation_required"`
  の順で、telemetry が 2 件とも出ているので**その先の代入まで到達している**
  ([video-stream.mjs:2372](../../crates/remote-web/web/video-stream.mjs))
- **+58.48 の時点で gate は立っていない。** `waiting_trigger="stalled"` なので
  watch を作ったのは `onStalled` → `beginWaiting("stalled")`。この関数は
  `awaitingUserActivation` を `videoPlaybackStallDecision` へ渡し、
  同関数は `cancel` を返す。**通ったということは gate が "ready" だった**
- watch を作る場所は `beginWaiting` の 1 箇所だけ (`playbackStallWatch =` は他に無い)
- `start()` は走っていない。走れば `playbackAttempts` が作り直されて `att` が 0 に戻るが、
  以後のサンプルも `att=2` のまま

**つまり +49.25 と +58.48 の間で、誰かが `playbackGate` を "ready" に戻している。**

## 4. 有力な手がかり

`playbackGate = "ready"` に戻す場所は 4 つある。そのうち 2 つが、
**同じ場所で autoplay の通知も消している**:

```js
this.playbackGate = "ready";
if (this.noticeKind === "autoplay") this.hideNotice();
```

- `onPlaying` ([video-stream.mjs:1664](../../crates/remote-web/web/video-stream.mjs))
- `setPlaying(false)` の else 枝 ([video-stream.mjs:2266](../../crates/remote-web/web/video-stream.mjs))

**利用者は「タップで再生の案内が出ない」と言っている。** 通知を出す経路
(`showNotice(..., "autoplay", "タップして再生", ...)`) は素通しで、
`showNotice` の抑止条件は `waiting` / `buffering` にしか掛からない。
**通知が見えなかったこと自体が、この 2 箇所のどちらかが走った証拠**ではないか。

**これは仮説であって確定ではない。検証して報告してほしい。**
`onPlaying` なら、拒否された直後に `playing` が飛ぶ経路を説明してほしい。
`setPlaying(false)` なら、`playRequested` が false になったのに
+61.48 の判定が `waiting` を返した (= `playRequested` が true) 整合も説明してほしい。
**どちらでもない第 3 の経路なら、それを教えてほしい。**

## 5. もう 1 つ気付いた点 (関係あるか判断してほしい)

`att=2` になっている。`playIfRequested` は
`playbackAttempts.pending > 0` と `playbackGate === "user_activation_required" && !userInitiated`
で二重に守られているのに、**1 回目の拒否の 20ms 後に 2 回目が走っている。**

`await this.video.play()` の拒否は microtask で再開するので、その後の gate 代入より前に
別の契機から `playIfRequested` が呼ばれると、両方の guard をすり抜ける余地がある。

**この競合が gate 解除の原因なのか、無関係な別の緩さなのか判断してほしい。**

## 6. やってほしいこと

1. **gate が解除される経路を特定する。** 記録を足さないと分からないなら、
   足すだけで止めてもよい。**推測で直さないこと**
2. 特定できたら、**自動再生の待ちが他の経路に解除されない**ようにする。
   ここは「再生する意図はあるが、ブラウザが許していない」という状態であり、
   利用者のタップ以外で抜けてはいけない
3. **タップで再生できる案内が確実に見えること。** 今は何も出ていない
4. この状態で停止監視が動かないこと

## 7. 守ること

- **症状を消す guard / retry / delay を根本原因の代わりに入れない**
- 状態が「再生中 / 停止 / 自動再生待ち」の 3 つに増えているので、bool を足して
  分岐を散らさない。**単一の所有者で表現すること**
- **回帰テスト**を付ける。gate の遷移は純関数側で固定できるはず。
  「拒否の後に他の経路が来ても gate が落ちない」ことを直接テストしてほしい
- web テストが緑

## 8. 注意

- ビルド (build-dev.ps1 / build-release.ps1) とコミットはしない。テストは走らせてよい
- 原因は分かったが正しい修正が広範囲になる場合は、直さずに報告してほしい
