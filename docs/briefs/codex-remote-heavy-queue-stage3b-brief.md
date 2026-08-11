# 表示所有権の cutover 段階 3b — 並べ替えられる queue にして、入口で断るのをやめる

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。

## 0. 前提 — 先に読むもの

- [`docs/web-remote-plan.md`](../web-remote-plan.md) **§14.9 / §14.10 / §14.11** (段階 1・2・3a の契約)
- `src/remote_ipc/heavy_queue.rs` — 段階 2 で用意した 3 レーン queue (dormant)。契約は §14.10
- `src/remote_ipc/page_jobs.rs` — registry。優先度の**正本**
- `src/remote_ipc/pipe.rs` — `try_acquire_prefetch` (2312 付近)、`enqueue_work`、`worker_loop`、
  `execute_work`、`QueueMetrics`、`work_lane`、`PageJobWork`
- `crates/remote-web/src/http.rs` — `MAX_CONCURRENT_IPC` 系 (50-53 行)

段階 3a で所有権は切り替わった。3b は**並べ替えと admission だけ**を扱う。

## 1. なぜこれで初めて拒否をやめられるか

現在の heavy lane は `sync_channel` の素の FIFO で、並べ替えられない。だから混雑への対処が
**入口で断る**しかなく、その判定 (`try_acquire_prefetch`) は `queued > 0` なら無条件で拒否する。
queue の中身が前景ページかサムネイルか先読みかを区別していないので、**サムネイルが 1 件
待っているだけで先読みが全部弾かれる**。実測で `/api/page` の 22-24% が 503 だった。

段階 3a までで、昇格 (`PageDemand` promote) と需要ベースの取消 (release → registry token) が
揃った。§14.10 に書いたとおり、**昇格と剪定が無いまま拒否をやめると悪化する**が、それは
もう無い。

**queue の深さは remote-web 側が既に縛っている** (`MAX_CONCURRENT_IPC = 6`、
`MAX_CONCURRENT_HEAVY_IPC = 4`、`MAX_CONCURRENT_PAGE_PREFETCH = 2`)。したがって本体の heavy
queue に同時に積まれるのは高々数件で、この増分は「大量の滞留を捌く」話ではない。
**前景 1 枚が先読み 1-2 件の後ろで数秒待つのをやめる**話であり、
**サムネイルが待っているだけで先読みが弾かれるのをやめる**話である。

## 2. 変更内容

### 2.1 heavy lane を `HeavyQueue` へ差し替える

- `mpsc::sync_channel::<Work>(HEAVY_WORK_QUEUE_CAPACITY)` と
  `Arc<Mutex<Receiver>>` を `Arc<HeavyQueue<HeavyKey, Work>>` に置き換える。
  **home / write / stream lane は現行のまま**。触るのは heavy だけ
- **key は `(connection_id, request_id)`**。request ID は connection 内で一意で、
  connection 単位の剪定にも使える。page job の `job_id` から key を引ける対応表を
  glue 側に持つ (registry にも queue にも持たせない。§14.10 の境界を崩さない)
- **lane 割り当て**: `Page` の実効優先度が Foreground なら `Foreground`、Prefetch なら
  `Prefetch`、それ以外の heavy (サムネイル、コンテナ列挙、AI、video jump) は `Interactive`
- **実効優先度は registry から取る** (3a で入れた `effective_page_priority`)。
  `PageRequest.priority` は初期値でしかない
- `dormant` のための `#[allow(dead_code)]` を外す

### 2.2 入口の拒否を撤去する

- **`try_acquire_prefetch` / `PrefetchPermit` / `prefetch_in_flight` /
  `MAX_CONCURRENT_PAGE_PREFETCH` (本体側) / `remote_page_prefetch_limit` を削除する。**
  同時実行の制限は queue の pop 条件 (`active < workers - 1`) が持つ
- **例外: worker が 1 本の環境。** `workers - 1 == 0` なので Prefetch は永久に pop されない
  (§14.10 の宿題)。**push の時点で断り、現行と同じ busy 応答を返す。** 待たせると
  `PAGE_RESPONSE_TIMEOUT` の 10 分まで HTTP worker と IPC 枠を掴む。断った事実は
  typed な理由付きでログに残す
- **remote-web 側の `IpcAdmission` は触らない** (HTTP worker を守る別目的)

### 2.3 剪定を配線する

- **release で queue に待機中の仕事は、その場で取り出して `Cancelled` を返す。**
  3a では pop されるまで枠を占有し、worker が起きてから短絡していた。動作は正しいが、
  枠が空くのが遅い。`prune` が返した payload には**必ず typed な応答を返す**
  (`Work` が client への `reply` を握っている)
- **接続断**は、その connection の待機中の仕事をまとめて剪定して `Cancelled` を返す。
  registry 側の `close_connection` と同じ場所で呼ぶ
- **shutdown** は `queue.shutdown()` が返す全 payload に停止応答を返す

### 2.4 lane 別の容量とログ

- `HEAVY_WORK_QUEUE_CAPACITY = 16` の単一上限をやめ、**lane 別**にする。先読みが溢れても
  前景の push が失敗しないことが要点 (§14.10)。remote-web が同時 4 件しか出さないので
  値そのものは余裕を持たせてよいが、**なぜその値かをコメントに書く**
- lane が満杯のときは現行と同じ busy 応答 (`queue_busy_response`) を返す
- **既存ログのフィールドを壊さない**。`queue=heavy` / `queued` / `active` / worker 名 /
  `queue_wait_ms` / `outcome` / `duration_ms` / `reply_ok` はそのまま残す。値は queue の
  snapshot から作り、**`QueueMetrics` と二重計上しない** (heavy については queue を単一の
  出所にする)。lane 別の内訳は**新しいフィールドとして足す**

## 3. 触らないもの

- home / write / stream lane、`worker_loop` 以外の worker 種別
- `remote_heavy_worker_count` の計算式 (`(設定値 / 2).clamp(1, 3)`)
- remote-web の `IpcAdmission` と `MAX_CONCURRENT_*`
- 段階 3a で入れた lease / registry / protocol (**protocol version は上げない**。
  wire 形式は変わらない)
- 先読み窓 12/4、予算 64 MiB、画質 preset、`page_display` telemetry
- 位置の requested / displayed 所有権 (§14.5.1)。**3c で扱う**

## 4. テスト

```
cd crates/remote-web/web && node --test
cp vendor/ffmpeg/bin/*.dll target/debug/deps/
cargo test -p mimageviewer --lib remote_ipc::
cargo test -p mimageviewer-remote
```

- 前景ページが、待機中のサムネイルと先読みより先に pop される
- **サムネイルが待機中でも先読みが拒否されない** (これが 503 の主因だった)
- 昇格した job が Foreground lane へ移り、prefetch として実行されない
- release で待機中の仕事が剪定され、**`Cancelled` が返る**
- 接続断でその connection の待機中の仕事だけが剪定され、別 connection は無傷
- worker 1 本の環境で先読みが**即座に断られる** (10 分待たない)
- lane が満杯のときだけ busy を返し、先読みが溢れても前景の push は成功する
- 既存ログのフィールドが全部残っている
- shutdown で待機中の全 payload に応答が返る

## 5. ドキュメント

`docs/web-remote-plan.md` に **§14.12** を追加する。

- 撤去したもの (`try_acquire_prefetch` / `PrefetchPermit` / 本体側 prefetch 上限) と、
  同時実行の制限が queue の pop 条件へ移ったこと
- **1 worker 環境で先読みを入口で断る決定と理由** (§14.10 の宿題への回答)
- 剪定した仕事へ typed 応答を返す義務
- lane 別容量にした理由と選んだ値の根拠
- **queue の深さは remote-web 側の admission が縛っており、この増分は滞留を捌く話ではない**
  という前提 (次に読む人が「もっと深い queue」を前提に設計しないように)
- ログの出所を queue の snapshot に一本化したこと

## 6. 実行と報告

- §4 のコマンドを**毎回実行**して結果を報告する
- **`src/` と `crates/` に触れた箇所は全部と、その理由を報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない**
- **実機確認が要る増分である。** 見るべき箇所を報告に列挙する。段階 3a で未確認のまま
  残っている 2 つ (予算満杯からの遠近交換 / 表示中の切断・再接続とセッション解放) も
  **この増分の実機確認に含める**
