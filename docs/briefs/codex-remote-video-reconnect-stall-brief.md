# mIV Remote: 切断後に再接続すると動画が「準備しています」から進まない

worktree: `C:\home\mimageviewer-web` / branch `web-remote` / 起点 `d67c1d18`

## 0. 立場

**本体が正本。独自の規則を発明しない。**
以下は私が読んで確認した内容だが、**実際と違っていたら実際の方を報告してほしい。**
私の要約に合わせて実装しないこと。この段取りで既に私の誤りが 18 回訂正されている。
直近でも、409 をセッション切断専用だと思い込んでいた私の前提を、あなたが訂正した。

**下の §3 は仮説であって確定ではない。裏を取ってから直してほしい。**

稼働中の本体 / remote-web は操作しない。`build-dev.ps1`・コミットも実行しない。

## 1. 症状 (利用者報告、実機)

`d67c1d18` の確認中に判明。**直前の変更で入った経路の可能性が高い。**

1. 端末で動画を再生
2. 本体の「切断する」を押す → **即座に音が止まりモーダルになる (ここまでは正しく動く)**
3. 端末の再接続ボタンを押す
4. → **「動画を準備しています。」から進まない**

利用者の観察: 「**本体側がモーダルダイアログになっていない**ので、再接続がうまく
認識されていなさそう」。

閲覧側の再接続 (動画でない通常操作) は前回の確認で通っている。**動画だけ**の問題。

## 2. 直前に入れた再接続経路

`d67c1d18` で以下を追加した:

- `applyRemoteSessionState` が切断時に `remoteSessionResume`
  (位置と再生状態) を記録する
  ([video-stream.mjs:1475](../../crates/remote-web/web/video-stream.mjs))
- 再接続ボタンが `resumeAfterRemoteSessionReconnect()` を呼び、
  `restartAt(positionSecs, restorePlaying)` する
  ([app.js:842](../../crates/remote-web/web/app.js))
- `restartAt` は旧 session を `/api/video/stop` して `start()` し直す
  ([video-stream.mjs:2486](../../crates/remote-web/web/video-stream.mjs))

## 3. 私の仮説 (裏を取ってほしい。違っていたら実際を報告)

本体側で切断すると、セッションは即座に終わらず **`RemoteControlPhase::DrainingRemote`**
に入り、**本体の UI フレームで排出が完了する** ([ui.rs:389-414](../../src/remote_ipc/ui.rs))。

そして**排出中は毎フレーム**リモートの動画ストリームを取り消している:

```rust
if remote_phase == RemoteControlPhase::DrainingRemote {
    self.cancel_remote_video_stream_state(
        VideoStreamErrorCode::SessionMismatch,
        "リモートセッションを終了しています",
    );
}
```

同じ関数のコメントが、まさにこの競合を認識している:

> an acquire and the first video start can both arrive before the next UI frame;
> cancelling after the drain would incorrectly reject that new owner's freshly opened stream.

**再接続と最初の動画開始が、排出完了前に届いた場合**に、新しい所有者のストリームが
取り消されているのではないか。利用者の「本体がモーダルになっていない」という観察は、
まだ排出中でリモート所有に戻りきっていない状態と整合する。

**この仮説が正しいか確認してほしい。** 違うなら実際の原因を報告してほしい。

## 4. もう 1 つ直してほしい — 待ち続けること自体

**原因が何であれ、「準備しています」のまま無限に待つのは避けたい。**

`STARTUP_MEDIA_SEGMENT_TIMEOUT_MS = 15000` があるのに止まらないなら、この経路が
その監視の外にある。取り消されたことが端末に届いていないか、届いても待ちを解いて
いない。

**何が起きたか分かる形で終わり、やり直せること。** 黙って待ち続けない。

## 5. 受け入れ条件

- 動画再生中に本体から切断 → 再接続 → **切断前の位置から再生が再開する**
- 再開できない事情があるときも、**「準備しています」で止まらず、状態が分かる**
- 本体側が排出中でも、再接続後のストリーム開始が取り消されない
  (あるいは取り消されたら端末がやり直す)
- 再接続直後に本体側が「リモート接続中」の状態へ正しく戻る
- 前回通った項目が退行していない:
  - 切断で即座に音が止まる / モーダルで塞がる
  - `Expired` からの自動復帰でモーダルが出ない
  - 一時的な通信断で塞がれない
  - 通常の閲覧・ページ送り
- **回帰テスト**を付ける。排出中に acquire と start が届く順序は、状態遷移テストで
  固定できるはず
- `cargo test -p mimageviewer --lib` / remote / ipc / web が緑

## 6. 注意

- ビルド (build-dev.ps1 / build-release.ps1) とコミットはしない。テストは走らせてよい
- **症状パッチを入れないこと。** delay や retry で見えなくするのではなく、
  所有権が戻る順序として正しい形にしてほしい
- `/stream/`、`/api/ai/jobs`、`/api/video/*` の認証・fail-closed guard を弱めない
- 原因は分かったが正しい修正が広範囲になる場合は、直さずに報告してほしい
