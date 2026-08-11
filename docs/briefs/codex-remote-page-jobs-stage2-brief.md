# 表示所有権の cutover 段階 2 — 本体側の基盤を dormant で追加する

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。

## 0. 前提 — 先に読むもの

- [`docs/web-remote-plan.md`](../web-remote-plan.md) **§14 / §14.3 / §14.9** (段階 1 で固定した契約)
- `crates/remote-web/web/page-coordinator.mjs` — 段階 1 の状態機械。**本体側はこの相手方になる**
- `src/remote_ipc/pipe.rs` — `try_acquire_prefetch` (2312 行付近)、`enqueue_work` (2290 行付近)、
  `worker_loop` (421 行付近)、`WorkLane` / `QueueMetrics` (60-113 行付近)、
  `remote_heavy_worker_count` (414 行付近)、`HEAVY_WORK_QUEUE_CAPACITY` (53 行)
- `src/remote_ipc/container.rs` — `begin_page_render` / `finish_page_render` (965-1005 行)、
  `page_inner` (2002 行付近)
- `src/pdf_loader.rs` — **参照実装**。`JobQueue` (1248 行付近) の 3 レーン + `Condvar` +
  `promote_to_high_normal_impl` (1422 行付近) + context epoch による剪定 (1489-1530 行付近)

**段階 2 も dormant である。** 新規モジュールを 2 つ足して単体テストで固定するだけで、
**request 経路からは呼ばない**。`try_acquire_prefetch` も `begin_page_render` も
`sync_channel` も worker 数もこの増分では変えない。protocol version も上げない
(上げるのは段階 3)。`cargo test` だけで検証でき、実機の挙動は変わらない。

## 1. いま本体側で何が起きているか (再調査不要、コードで確認済み)

| 事実 | 場所 |
|---|---|
| heavy lane は `sync_channel<Work>` の素の FIFO。worker が `Mutex<Receiver>` から直接 `recv` する | `pipe.rs` 205-211、421-440 |
| 並べ替えができないので、混雑への対処が**入口で断る**しかない。それが 503 の正体 | `try_acquire_prefetch` → `queue_busy_response` |
| その入口判定は `queued > 0` なら**無条件で拒否**する。queue の中身が前景ページかサムネイルか先読みかを区別していない | `pipe.rs` 2321 |
| `begin_page_render` の「要らない先読みを取り消す」走査は **worker の中** = 既に走り出した後に呼ばれる | `container.rs` 2037、`page_inner` から |
| したがって **queue で待っている先読みは、後から来た前景では剪定されない**。順番が来れば必ず走る | 同上 |
| 取消の判断は住所と `spread_partner` の一致で近似している | `container.rs` 977-992 |
| worker 数は `(設定値 / 2).clamp(1, 3)`、先読みは `min(worker-1, 2)` | `pipe.rs` 414-419、2334 |

**3 レーン化するだけで拒否をやめると今より悪化する。** 昇格と剪定が無いぶん、読者が先へ
進んだ後の古い先読みが queue に溜まり、前景がその後ろで待つ。昇格と剪定は段階 1 で契約を
固めた D0 そのものなので、**基盤 (この増分) → 一体で cutover (段階 3)** の順に進める。

## 2. 追加するもの

新規 2 モジュールと、`src/remote_ipc/mod.rs` への `mod` 宣言だけ。どちらも
**request 経路から呼ばない**ので、`#[allow(dead_code)]` を module 単位で付け、
「段階 3 (plan §14.3 の B + C + D0) で配線する」とコメントに書く。

### 2.1 `src/remote_ipc/page_jobs.rs` — ページジョブ registry

段階 1 の coordinator が発行した job を本体側で受け止める登録簿。**住所の近似ではなく
ID で照合する**のが要点で、`begin_page_render` の retain を置き換える相手になる。

```rust
pub struct PageJobId(String);         // Web coordinator が発行した job ID
pub struct DisplayRequestId(String);  // 同じ表示グループの全ページが共有する

pub enum PageJobPriority { Prefetch, Foreground }   // 単調にしか上がらない

pub enum PageJobCancelCause {
    NoDemand,            // 読者が先へ進み、需要が空になった
    SessionInvalidated,  // session 失効
    ConnectionClosed,    // 接続が切れた
    ServiceStopping,     // 本体側の停止
}

pub struct PageJobRegistry { /* Mutex 内に記録 */ }
```

操作 (すべて connection 単位に scope する):

| 操作 | 返り値 |
|---|---|
| `register(connection, job, display_request, priority)` | ジョブ固有の `Arc<AtomicBool>` cancel token。既知 ID の再登録は typed な拒否 |
| `promote(connection, job)` | `Promoted` / `AlreadyForeground` / `UnknownJob`。**Prefetch → Foreground の一方向のみ** |
| `release(connection, job, cause)` | 冪等。cancel token を立てる。未知 ID は typed な no-op |
| `finish(connection, job, outcome)` | 記録を外す。release 済みなら `AlreadyReleased` |
| `cancel_connection(connection, cause)` / `cancel_all(cause)` | 切断・session 失効・停止で使う |
| `snapshot()` | log 用の集計 (lane 別 / 状態別の件数) |

固定する不変条件 (§14.9 の Web 側契約と対になる):

1. **打ち切りは明示的な入口だけから起きる。** 住所や `spread_partner` の一致で他のジョブを
   巻き添えにしない。`begin_page_render` の近似を持ち込まないこと
2. **昇格は単調で高々 1 回。** 降格する API を作らない
3. **release は冪等**で、二重解放が二重に効かない
4. **connection ごとに閉じる。** ある connection の操作が別 connection の記録へ影響しない
   (CLAUDE.md「context 固有の resource は所有 context だけに作用する」)
5. **無言の no-op を作らない。** 未知 ID・二重解放・昇格済みは、すべて typed な結果として
   返す。段階 1 の `ignored{reason}` と同じ方針で、原因がログから見えるようにする

### 2.2 `src/remote_ipc/heavy_queue.rs` — 優先度付き heavy queue (dormant)

`sync_channel` の置き換え先。**payload はジェネリック `T`** にしておき、段階 3 で
`Work` を載せる。参照実装は `pdf_loader.rs` の `JobQueue`。ただし本体の PDF プールと違い
worker が自分で pop するので、専用 dispatcher スレッドは要らない。

- **3 レーン**: `Foreground` (読者が待っているページ) / `Interactive` (サムネイル・
  コンテナ列挙など、読者が待っている一覧) / `Prefetch` (先読みページ)。
  pop は Foreground → Interactive → Prefetch の順、同一レーン内は FIFO
- **容量はレーンごとに持つ。** 全体で 1 本の上限にすると、先読みが溢れたときに前景の push が
  失敗して 503 が戻ってくる。今の `HEAVY_WORK_QUEUE_CAPACITY = 16` は全体 1 本なので、
  そこを構造で断つ
- **前景用の予約**: `Prefetch` は `active < workers - 1` のときだけ pop してよい。前景が
  到着したとき必ず空き worker が 1 本ある状態を保つ。これが `try_acquire_prefetch` の
  入口判定を queue の中へ移したもの (**この増分では入口判定は撤去しない**)
- **`promote(key)`**: queue で待っている項目を Prefetch から Foreground へ移す。冪等で、
  動かしたかどうかを返す。実行中・未知の項目は typed な no-op
- **`prune(predicate)`**: 待機中の項目を落とす。**落とした項目は呼び出し側へ返す。**
  `Work` は client への `reply` を握っているので、黙って捨てると要求が永久に返らない。
  破棄した項目に typed な応答を返すのは呼び出し側の責務であり、それを可能にする形にする
- `Mutex + Condvar` で blocking pop、`shutdown()` で待機中の worker を全部起こす。
  **完了時も notify する** (予約で pop を見送った worker が、空きができたときに起きられないと
  停止する)
- レーン別の queued / active を snapshot として出す (今の `QueueMetrics` 相当のログ用)

### 2.3 registry と queue の境界 (段階 3 で効いてくるので、ここで決めておく)

- **queue が持つのは順序だけ** — 次に何を走らせるか、どのレーンか、予約を満たすか
- **registry が持つのは需要と生死** — ジョブの存在、優先度、打ち切りとその理由
- 優先度の正本は **registry**。queue のレーンはその写しであり、`queue.promote()` だけが
  レーンを変える。段階 3 の配線側が registry と queue を**同じ critical section で**
  揃える。両者とも冪等なので、途中で失敗しても次の昇格で追いつく

この境界を崩して「registry が queue を直接操作する」形にしないこと。2 つが相互に相手を
呼ぶと、段階 3 で lock 順序を決められなくなる。

## 3. 触らないもの

- `pipe.rs` の request 経路 (`try_acquire_prefetch` / `enqueue_work` / `work_lane` /
  `sync_channel` / `worker_loop`)。**この増分では 1 か所も配線しない**
- `container.rs` の `begin_page_render` / `finish_page_render` / `page_inner`
- `crates/remote-ipc` の protocol 型と **`PROTOCOL_VERSION`**。`PageRequest` へ
  `display_request_id` / `job_id` を足すのは段階 3
- worker 数・先読み上限・queue 容量の**現行値**。値の調整は段階 3 で入口を撤去するときに
  まとめて判断する (`remote_heavy_worker_count` / `MAX_CONCURRENT_PAGE_PREFETCH` /
  `HEAVY_WORK_QUEUE_CAPACITY`)
- `crates/remote-web` 側 (`ipc_client.rs` / `http.rs` の 503 変換)
- `crates/remote-web/web/` の JS 一式 (段階 1 で確定済み)

## 4. テスト

`src/remote_ipc/` の既存テストと同じく `#[cfg(test)] mod tests` をモジュール内に置く。

**registry**

- 昇格は Prefetch → Foreground の 1 回だけ。2 回目は `AlreadyForeground`、未知は `UnknownJob`
- 降格の手段が無い (API として存在しない)
- release は冪等。二重解放しても cancel token は 1 回しか立たず、2 回目は typed な no-op
- release 後の `finish` は `AlreadyReleased` を返す
- `cancel_connection` は対象 connection の全ジョブだけを打ち切り、**別 connection の記録を
  1 件も変えない**
- 同じ `job_id` を別 connection が使っても互いに衝突しない
- 未知 ID・二重操作が**すべて typed な結果**で返る (無言の no-op が無い)

**queue**

- 同一レーン内は FIFO。レーン間は Foreground → Interactive → Prefetch
- 前景の push は、Prefetch レーンが満杯でも成功する (レーン別容量)
- worker が N 本のとき、Prefetch だけが積まれていても同時実行は N-1 本まで。
  **1 本完了すると残りが開始する** (通知の取りこぼしが無い)
- 前景が到着したとき、必ず空き worker が 1 本ある
- `promote` は待機中の項目を移し、冪等。実行中・未知は typed な no-op
- `prune` は落とした項目を呼び出し側へ返す (黙って捨てない)
- `shutdown()` で blocking pop 中の worker が全員起きる
- レーン別 snapshot が実際の件数と一致する

**実行するコマンド** (`cargo test -p mimageviewer --lib` は事前に FFmpeg DLL のコピーが要る。
plan §13.3 参照)

```
cp vendor/ffmpeg/bin/*.dll target/debug/deps/
cargo test -p mimageviewer --lib remote_ipc::
cargo test -p mimageviewer-remote
```

非 Windows CI (`cargo check` on ubuntu) が通るよう、新規モジュールは `std` だけで書き、
`cfg(windows)` を必要とする API を使わないこと。

## 5. ドキュメント

`docs/web-remote-plan.md` に **§14.10** を追加し、次を記録する
(`docs/briefs/` は git 管理外なので、決定はここへ書き戻さないと次のセッションが逆を実装する)。

- 段階 2 で足した 2 つの基盤と、**registry / queue の役割境界** (§2.3)。優先度の正本は
  registry であり、レーンはその写しであること
- **なぜ 3 レーン化だけでは足りないか** — 昇格と剪定が無いまま入口の拒否をやめると、
  古い先読みが queue に溜まって前景がその後ろで待つ。だから D0 と同時に切り替える
- **今の `begin_page_render` が worker の中で走るため、queue で待っている先読みを剪定
  できない**という事実 (段階 3 で入口が enqueue 側へ移る根拠)
- 剪定した項目を捨てずに呼び出し側へ返す理由 (`Work` が client への `reply` を握っている)
- 前景用予約を「入口の拒否」から「queue の pop 条件」へ移す設計であること。
  **ただしこの増分では入口の拒否を撤去していない**
- この増分が dormant であること (request 経路から未使用、protocol version 据え置き、
  実機の挙動は変わらない)

ユーザー向けマニュアル (`htdocs/`) は変更しない。

## 6. 実行と報告

- §4 のコマンドを**毎回実行**して結果を報告する
- **`src/` に触れた箇所は全部と、その理由を報告する** (今回は新規 2 モジュールと
  `mod.rs` の宣言だけの想定。それ以外に触れたなら理由を明記)
- **`scripts/build-dev.ps1` を実行しない** (稼働中の本体と remote サービスを止めてしまう)。
  ビルドは ClaudeCode 側で行う
- **コミットしない** (ClaudeCode が行う)
- 段階 3 の配線で問題になりそうな点 (lock 順序、`Work` の所有、`session_operation` /
  `session_cancel` との合成、metrics ログの互換) に気付いたら、実装せず**報告する**
