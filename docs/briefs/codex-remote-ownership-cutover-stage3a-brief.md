# 表示所有権の cutover 段階 3a — 需要の所有権を Web と本体で同時に切り替える

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。

## 0. 前提 — 先に読むもの

- [`docs/web-remote-plan.md`](../web-remote-plan.md) **§14 / §14.2 / §14.3 / §14.5 / §14.9 / §14.10**
- `crates/remote-web/web/page-coordinator.mjs` — 段階 1 の状態機械 (契約は §14.9)
- `src/remote_ipc/page_jobs.rs` — 段階 2 の registry (契約は §14.10)
- `crates/remote-web/web/app.js` — `PageResourceCache` (450-760 行)、`imageRequest` (5873 行付近)、
  `loadMeasuredImage` / `loadMeasuredSpread` (11194 / 11367 行付近)、
  `AdjustmentPanel.runPreview` (8500 行付近)
- `src/remote_ipc/pipe.rs` — `try_acquire_prefetch` (2312)、`enqueue_work` (2290)、
  `worker_loop` (421)、`execute_work` (1064)
- `src/remote_ipc/container.rs` — `begin_page_render` / `finish_page_render` (965-1005)、
  `page_inner` (2002)
- `crates/remote-web/src/http.rs` — `api_page` (3149)、`IpcAdmission` (186 付近)
- `crates/remote-web/src/ipc_client.rs` — `PAGE_RESPONSE_TIMEOUT` (42)

## 1. この増分の範囲 (段階 3 を 3a / 3b に分ける)

plan §14.3 は B + C + D0 を一体の cutover と決めている。**その一体性は「取消の所有者を
2 つ並存させない」ことにあり、admission (入口で断るかどうか) はそれとは別の軸である。**
そこで段階 3 を次の 2 つに割る。

- **3a (この増分) — 取消と優先度の所有権を切り替える。** Web の表示グループ lease、
  job ID による本体側の登録・昇格・明示 release、`begin_page_render` の住所近似の撤去、
  `loadForeground` の他 active 取消走査の撤去、補正プレビューの統合、protocol 版上げ。
  **heavy queue は現行の `sync_channel` のまま、`try_acquire_prefetch` の入口拒否も残す**
- **3b (次の増分) — 段階 2 の `heavy_queue` へ差し替え、入口拒否を撤去する。** 剪定と
  昇格が 3a で揃っているので、そこで初めて拒否をやめられる

**割る理由**: 一度に変えると、実機で不具合が出たとき「所有権の配線ミス」か「並べ替えの
副作用」かを切り分けられない。3a は 503 の発生率を変えないので、比較すべき軸が 1 本になる。
plan §14.3 の記述をこの分割で**置き換える** (§13.4.1 の運用に従い、古い記述は差し替え済みと
明記する)。

## 2. 何が起きているか (再調査不要)

段階 1・2 で確定済みの事実に加えて、3a で効くもの:

| 事実 | 場所 |
|---|---|
| ブラウザの `abort` は本体の処理を止めない。**明示的な release だけが止められる** | §14.1 |
| `PAGE_RESPONSE_TIMEOUT` は **10 分**。応答を返さない仕事は HTTP worker と IPC 枠を 10 分掴む | `ipc_client.rs` 42 |
| remote-web 側にも admission がある (`IpcAdmission`、heavy 4 / prefetch 2)。これは **HTTP worker の枯渇を防ぐ別目的**であり、本体側の入口拒否とは別物 | `http.rs` 186 付近 |
| 補正プレビューは cache / coordinator の外から前景 `/api/page` を送り、`signal` を渡していないので中止手段が無い | `app.js` `runPreview` |
| session の drain は各 operation の cancel flag を立てる。page job の token とは別物 | `session.rs` 632-637 |
| 接続断は `remote_web_disconnected(connection_id)` を通る | `session.rs` 1601 |

## 3. 決めたこと (段階 2 が残した宿題への回答)

1. **先読みの job には display request ID が無い。** 段階 1 の coordinator は計画由来の
   `start` に `requestId` を載せない。よって **wire 上は optional** とし、registry の
   `display_request_id` を `Option` にする。**昇格したときに、昇格させた display request ID を
   記録する** (相関が意味を持つのはその瞬間なので、field は死なない)
2. **昇格した仕事の実効優先度は registry が正本。** `PageRequest.priority` は初期値でしかない。
   pipe の glue が dispatch 直前に registry から解決し、`container.rs` へ渡す。
   container は registry を知らない
3. **捨てる仕事には必ず typed な応答を返す。** release / 接続断 / 停止で仕事を落とすときは、
   新設する `MediaErrorCode::Cancelled` を返して HTTP worker を解放する。`SessionOperation` を
   drop するだけでは client へ返らない (10 分掴む)
4. **render から見える取消源を 1 つにする。** registry が発行した token を render へ渡し、
   session drain は `registry.cancel_all(SessionInvalidated)`、接続断は
   `registry.close_connection(ConnectionClosed)` を**同じ場所で**呼ぶ。
   `SessionOperation::cancel_flag()` は page 以外の仕事のためにそのまま残す
5. **release が GET を追い越す競合を塞ぐ。** release が先に着いて job が未登録だと、後から
   来た GET が登録して**そのまま走ってしまう**。registry に **connection ごとの有界な
   released-job 墓標**を持ち、`register` がそれを見て**最初から立った token を返す**。
   promote の追い越しは、失われても「先読みのまま走る」だけなので best-effort とし、
   その旨をコメントに残す
6. **1 worker 環境の実行不能な先読み**は 3b の宿題のまま。3a では入口拒否が残るので発生しない
7. **remote-web 側の `IpcAdmission` は触らない。** HTTP worker を守る別目的であり、
   前景は heavy 4 枠に対して最大 2 ページ + プレビューなので詰まらない

## 4. 変更内容

### 4.1 protocol (`crates/remote-ipc`)

- `PageRequest` に `job_id: String` と `display_request_id: Option<String>` を追加
- **新 message `ClientMessage::PageDemand`** — 1 回で `promote: Vec<String>` と
  `release: Vec<(String, cause)>` を運ぶ。**入口を増やさず 1 本の typed request に集約する**
  (CLAUDE.md「相互排他な状態を複数の入口で表現しない」)。応答は各 job の typed 結果を返す
- `MediaErrorCode::Cancelled` を追加
- **`PROTOCOL_VERSION` を 37 → 38 へ**。plan §13.5 の版数記述も更新する

### 4.2 本体 (`src/remote_ipc/`)

- `PageJobRegistry` を engine / session が所有する 1 インスタンスとして持つ
- `pipe.rs`: `ClientMessage::Page` を enqueue する**直前に** `registry.register(...)` し、
  得た token を `Work` に載せる。dispatch 直前に実効優先度を解決する。
  `PageDemand` は heavy queue を通さず**即座に**処理する (待たせると昇格・解放が遅れて
  意味が無い)。**register が pre-cancelled token を返したら、queue へ入れずに
  `Cancelled` を即応答する**
- `container.rs`: **`begin_page_render` / `finish_page_render` / `page_prefetches` /
  `ActivePagePrefetch` を削除**。`page_inner` は渡された job token を使う。住所と
  `spread_partner` による取消の近似はここで消える
- 接続断 (`remote_web_disconnected`) と session drain (`begin_drain`) から registry を呼ぶ
- ログ: 既存の `queued` / `active` / `queue=heavy` / worker 名 / 所要時間 / outcome を保つ。
  registry の snapshot は**別フィールドとして足す** (二重計上しない)

### 4.3 remote-web (`crates/remote-web/src/`)

- `/api/page` に `job` と `display` の query を追加。検証は既存の generation / epoch と同じ
  文字種・長さ制限に揃える
- **新 `POST /api/page/demand`** — `{promote: [...], release: [{job, cause}]}` を受けて
  `PageDemand` を送る。**heavy でも prefetch でもない軽い class** で扱い、page の枠を消費しない
- `MediaErrorCode::Cancelled` は **HTTP 409** + `miv_media_error` の typed code とする。
  **503 にしない** (Web 側の busy 再試行に乗せてはいけない)

### 4.4 Web (`crates/remote-web/web/app.js`)

段階 1 の `PageDisplayCoordinator` を唯一の需要 owner として配線する。

- `updateViewerImage` が表示グループを開くときに `coordinator.nextDisplayRequestId()` を取り、
  **fetch 開始前に同期で** `openDisplay({requestId, groupKey, keys})` する
- `imageRequest` の `cacheKey` を **`pageResourceKey()` に置き換える** (段階 1 で固定した
  完全なキー)。URL には `job` と `display` を載せる
- **撤去する**: `loadForeground` の「他の active を打ち切る」走査、`foregroundWaiters`、
  `prefetchPlanned`、`abortUnownedActive`。`PageResourceCache` は
  **バイトの保持・予算・LRU だけ**を持つ owner になる
- `PageResourceCache` の保護集合は `coordinator.protectedKeyIds()` から取る
  (`visibleKeys` を置き換える)
- coordinator の effect を実行する adapter を 1 本置く。`start` → fetch 開始、
  `promote` / `cancel` → `POST /api/page/demand` へ**同一 tick でまとめて**送る、
  `group_ready` / `group_failed` → 段階 A の outcome へ繋ぐ、`ignored` → telemetry
- **補正プレビューを同じ coordinator に通す** (要素数 1 の consumer)。`signal` を渡し、
  中止できるようにする
- `URL / prefetch=1` は 3a では**残す** (本体が `PagePriority` を決める入口として使う)

## 5. 触らないもの

- `src/remote_ipc/heavy_queue.rs` と `try_acquire_prefetch` / `sync_channel` / worker 数 /
  queue 容量。**3b で扱う**
- remote-web の `IpcAdmission`
- 位置の requested / displayed 所有権と URL / history の残存不整合 (§14.5 / §14.5.1)。
  **3c で扱う。**この増分で中途半端に位置を動かさない
- 先読みの窓 (12/4)、予算 64 MiB、画質 preset、`page_display` telemetry の既存フィールド
- 段階 A の `applied` / `superseded` / `failed` outcome 契約

## 6. テスト

```
cd crates/remote-web/web && node --test
cp vendor/ffmpeg/bin/*.dll target/debug/deps/
cargo test -p mimageviewer --lib remote_ipc::
cargo test -p mimageviewer-remote
```

**Web**

- 表示グループを開くと、fetch 開始前に全ページの lease が登録されている
- 表示要求が追い越されると、負けた要求の lease が解放され、**他の要求が必要としている
  ページは打ち切られない** (§14 の 3 件の再発防止)
- 先読み中のページに表示需要が付くと `promote` が 1 回だけ送られる
- 需要が空になったページだけが `release` される。release は同一 tick でまとめて 1 リクエスト
- 補正プレビューが coordinator を通り、中止できる
- `Cancelled` (409) を busy 再試行として扱わない

**本体**

- register → promote → release で token が立ち、`page_inner` が途中で止まる
- **release が GET より先に着いても、後から登録した job が最初から取り消されている**
- 接続断で当該 connection の job だけが取り消され、他の connection は無傷
- session drain で全 job が取り消される
- 実効優先度が registry から解決され、昇格済みの仕事が prefetch として実行されない
- 落とした仕事に `Cancelled` が返り、HTTP worker が解放される
- 既存ログのフィールドが失われていない

**remote-web**

- `/api/page` の `job` / `display` 検証 (不正は 400)
- `POST /api/page/demand` が promote / release をまとめて送れる
- `Cancelled` が 409 + typed code になる

## 7. ドキュメント

`docs/web-remote-plan.md` に **§14.11** を追加し、次を記録する。

- **段階 3 を 3a / 3b / 3c に割った決定と理由** (§1)。§14.3 の該当記述を差し替え済みと明記
- §3 の 7 つの判断 (特に optional な display request ID、実効優先度の正本、
  捨てる仕事への typed 応答、取消源の一本化、**release と GET の追い越しを墓標で塞ぐこと**)
- 撤去したもの (`begin_page_render` の住所近似、`loadForeground` の取消走査、
  `foregroundWaiters` / `prefetchPlanned` / `abortUnownedActive`) と、
  **それらが 1 つの lease に置き換わったこと**
- `PageResourceCache` がバイトの保持だけの owner になったこと
- protocol 38 の内容。§13.5 の版数記述も更新する

## 8. 実行と報告

- §6 のコマンドを**毎回実行**して結果を報告する
- **`src/` と `crates/` に触れた箇所は全部と、その理由を報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない** (ClaudeCode が行う)
- **これは実機確認が要る増分である。** 迷った箇所、実機で見るべき箇所を報告に列挙する
- 設計から外れた判断は理由とともに報告する
