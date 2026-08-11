# mIV Remote: 再接続しても本体が「リモート接続中」に戻らない

worktree: `C:\home\mimageviewer-web` / branch `web-remote` / 起点 `6e8b7bd8`

## 0. 立場

**本体が正本。独自の規則を発明しない。**
以下は私が読んで確認した内容だが、**実際と違っていたら実際の方を報告してほしい。**
私の要約に合わせて実装しないこと。この段取りで既に私の誤りが 18 回訂正されている。
**直前の動画 stall でも、私の仮説 (DrainingRemote の取り消し順) は外れで、
あなたが特定した実際の原因 (streaming worker の resource lease) が正しかった。**

**今回は原因を特定できていない。§2 は候補の列挙であって仮説ですらない。**

稼働中の本体 / remote-web は操作しない。`build-dev.ps1`・コミットも実行しない。

## 1. 症状 (利用者報告、実機。`6e8b7bd8` 適用後も再現)

動画再生中に本体から切断 → 端末で再接続すると、
**本体側が「リモート接続中」に戻らない。**

`6e8b7bd8` で streaming worker の lease は直ったが、この症状は残っている。
**別の原因。**

## 2. 分かっていること

本体側の取得完了はここで止まる ([ui.rs:374](../../src/remote_ipc/ui.rs)):

```rust
if remote_phase == RemoteControlPhase::AcquiringRemote
    && self.local_ai_remote_barrier_quiesced()
{
    handle.finish_acquire(snapshot.generation);
}
```

`AcquiringRemote` の本体表示は **「リモート接続の準備中」**
([ui.rs:1962](../../src/remote_ipc/ui.rs))。利用者の言う「リモート接続中に戻らない」が
この表示のことなら、**barrier が開いていない**。

`local_ai_remote_barrier_quiesced` ([app.rs:49017](../../src/app.rs)) は
**8 つの条件の AND**:

- `mounted_quiesced` / `detached_quiesced`
- `ai_upscale_pending.is_empty()`
- `retained_final_ai_orphans.is_empty()`
- `local_adjust_segmentation_pending.is_none()`
- `book_op_pending.is_none()`
- `local_ai_activity == 0`
- `!trt_restart_in_flight`
- `video_quiesced` (`video_upscale_running` が None か `paused_idle`)

**どれが false なのか観測する手段が無い。** 私からは絞り込めない。

気になっている点 (確認してほしいが、決め打ちしないこと):

- `begin_local_ai_remote_barrier` ([app.rs:48988](../../src/app.rs)) は
  `ai_upscale_pending` に cancel を立てるだけで**要素を消さない**。
  worker 側が終了時に消す作りなら、消えない経路があると永久に開かない
- `release_local_ai_remote_barrier` は `resume_video_upscale` で
  **動画アップスケールを再開する**。切断で解放 → 再接続で再度止める、の往復で
  停止しきる前に次の取得が来ないか
- lease は `remote_session_ui.local_ai_lease = Some(...)` で**上書き**される。
  前の lease が残っている状態で上書きすると取りこぼさないか

## 3. やってほしいこと

### (a) まず見えるようにする

**8 条件のどれが barrier を閉じているかをログに出せるようにしてほしい。**
今の作りでは、この種の停止が起きるたびに推測することになる。

停止が続いている間だけ (例えば数秒開かなかったら) 出す形でよい。毎フレーム
出す必要はない。**利用者の実機で 1 回再現すれば原因が特定できる**状態にしたい。

### (b) 原因を特定して直す

(a) で得られる情報、または再現手順から特定してほしい。
**§2 の候補に飛びつかないこと。** 私は絞り込めていない。

### (c) 開かない barrier を永久に待たない

**8 条件の AND を無期限に待つ構造そのものが危うい。**
1 つでも解けない条件があると、リモートは二度と操作権を取れない。

今回の原因を直したうえで、**開かないまま時間が経った場合にどうするか**を
判断して報告してほしい。強制的に進めるのか、諦めて明示エラーにするのか、
そもそも AND を待つ設計を見直すのか。**利用者が復旧できる形**であること。

## 4. 利用者に確認してもらう予定 (回答が来たら伝える)

- 本体側の表示が「**リモート接続の準備中**」で止まっているか、
  それとも通常のローカル表示に戻っているか
- 動画を再生していない状態で切断 → 再接続した場合も同じか
  (動画固有か、切断・再接続そのものの問題かの切り分け)

**この回答を待たずに (a) は進めてよい。** 観測手段は何にせよ要る。

## 5. 受け入れ条件

- 動画再生中に本体から切断 → 再接続 → **本体が「リモート接続中」に戻る**
- 端末側も通常どおり操作でき、動画が切断前の位置から再開する
- barrier が開かない場合、**どの条件が閉じているかがログから分かる**
- 開かないまま放置されても、**利用者が復旧できる**
- 前回通った項目が退行していない (切断で即停止 / モーダル / `Expired` 自動復帰 /
  一時的通信断で塞がない / 通常の閲覧)
- **回帰テスト**を付ける
- `cargo test -p mimageviewer --lib` / remote / ipc / web が緑

## 6. 注意

- ビルド (build-dev.ps1 / build-release.ps1) とコミットはしない。テストは走らせてよい
- **症状パッチを入れないこと。** barrier を条件から外す、待ち時間を延ばす、
  無条件に `finish_acquire` する、といった形で見えなくしない
- 原因は分かったが正しい修正が広範囲になる場合は、直さずに報告してほしい
