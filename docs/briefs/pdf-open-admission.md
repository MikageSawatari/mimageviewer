# PDF: open と render を別の資源として扱う (前面優先 + 同時 open 上限)

対象は backlog の §2.15 と §2.16 ([next-release-backlog.md](../next-release-backlog.md))。
2 つは同じ 1 つの機構で実現するので、まとめて 1 件として扱う。

着手前に読むもの:

- [CLAUDE.md](../../CLAUDE.md) の「バグ修正の一般原則」「並行処理: try_lock + sleep は使わない」
- [docs/async-architecture.md](../async-architecture.md) §5.5 (PDF pool の構造)
- [docs/pdf-pool-context-epoch-plan.md](../pdf-pool-context-epoch-plan.md)

## 1. 何が壊れているか

利用者報告 (2026-08-20): 「複数ウィンドウを開こうとしているとき、ダブルクリックで画像が
なかなか開かない」。3 回クリックして待ちが 1.5 → 2.2 → 3.7 秒と積み上がった。

利用者実機の perf log を `worker_open_ms` で分離した結果:

| 全 render の `worker_open_ms` | p50 | p90 | p99 | 最大 |
| --- | ---: | ---: | ---: | ---: |
| | **0.3 ms** | 763 ms | 5,730 ms | **10,803 ms** |

1 秒超の open が 75 件あり、**固まって発生していた**。区間の中身は
**別々の PDF を一斉に開いているところ** (フォルダのカバーサムネイル生成)。対象は HDD 上に
あり、5 ワーカーが同じディスクを取り合っていた。

**遅いのはファイルではなく状況**である。同じ `20221030_001.pdf` は他の時点で **0.4 ms** で
開いている (数十サンプル)。当初「病的なファイル」と誤診し、利用者の指摘で訂正した。

7.4 秒かかった 1 件の内訳:

```
worker_open_ms   = 7371.8   ← 競合を全部吸収している
worker_page_ms   =   11.5
worker_render_ms =  154.9   ← 平常時 (100〜170ms) と変わらない
```

**競合の最中でも描画時間は動かない。**

- **open はディスク律速** — HDD で同時に走らせるとシークが往復して待ちが跳ねる
- **render は CPU 律速** — 並列度を上げた分だけ効く

**性質が正反対の 2 つを、同じ 1 つの並列度で縛っているのが現状の構造。**

**Critical レーンの予約はワーカーを 1 つ確保するだけで、ディスクは確保しない。**
他の 4 ワーカーが大きな PDF を読んでいる間、予約されたワーカーも同じディスクを待つ。

## 2. 設計原則 (利用者、2026-08-21)

**利用者の直接の操作に応答することが最優先。** 資源が競合したら、背景仕事より前面を通す。

## 3. 採る構造 — open を独立した IPC 要求にする

**`MSG_OPEN` を新設し、「文書を開く」を仕事本体から切り離す。**

現在 `run_dispatcher` は pop した job を 1 往復の IPC で送るだけで、その 1 往復の中に
open と render の両方が入っている ([pdf_loader.rs:2714](../../src/pdf_loader.rs:2714))。
これを次に変える。

1. dispatcher は pop の時点で、**その job がこのワーカーで open を必要とするか**を判定する。
   ワーカーが既に同じ文書を保持していれば不要 (§2.13 で入れた
   `PdfDocumentCache` [pdf_loader.rs:480](../../src/pdf_loader.rs:480) が 1 冊を保持している)。
2. open が必要なら **open 許可枠を 1 つ取ってから pop する**。取れなければ pop しない。
3. pop したら、まず `MSG_OPEN` を 1 往復送る。**返ってきた時点で許可枠を返す。**
4. 続けて本来の要求 (Render / Enumerate / GetInfo / AnalyzePage) を送る。
   このときワーカーは文書を保持済みなので、**その要求の中で open は起きない。**

**許可枠は open の往復だけを覆う。render の間は枠を持たない。**
これが「render の並列度は保ったまま、同時 open 数だけを絞る」の実体である。

### 3.1 なぜ「1 往復のまま予測で数える」ではないのか

素朴な案は「job 単位で許可枠を取り、返信で返す」だが、**これは render の並列度も一緒に
絞ってしまう**。SSD では open 0.3 ms に対し render は数十〜150 ms なので、枠の 99% 以上が
render に食われ、サムネイル生成の同時実行数が既存の lane cap (Normal は `worker_count-2`、
HighNormal は `worker_count-1`) より下がる。**HDD の遅延を SSD の throughput で買う取引に
なるので採らない。**

もう 1 つの案は「ワーカーが open 完了を途中フレームで通知する」だが、
`send_recv_io` ([pdf_loader.rs:1720](../../src/pdf_loader.rs:1720)) は最大 190 MB を運ぶ
最ホット経路で、しかも `collect_metrics` の測定区間がその読み出しを包んでいる。
ここに前置フレームを挿すと測定の意味が変わる。**採らない。**

`MSG_OPEN` 方式は追加コストが 1 往復のフレーム送受 (数百 µs 未満) だけで、
**親が予測ではなく事実としてワーカーの保持文書を知れる**という副次的な利点がある。

### 3.2 open と本要求は分割しない

pop した後の `MSG_OPEN` → 本要求は **1 つの不可分な dispatch として扱う**。
間に cancel / epoch チェックを入れない。現在も 1 往復の中で open と render をまとめて
払っており、cancel されても払った分は変わらない。**ここは挙動を変えない。**

`MSG_OPEN` が失敗したら (パスワード誤り / ファイル消失など)、本要求を送らずに
そのエラー応答をそのまま requester へ返す。**エラー文字列は本要求経由と同一でなければ
ならない** — 通常 PDF オープンは Err 文字列に `Password` が含まれるかで分岐している
([app.rs:22605](../../src/app.rs:22605))。どちらの経路も `CachedPdfDocument::open` →
`pdfium_open_error` を通るので同一になるはずだが、**確認すること**。

## 4. 実装

### 4.1 文書 identity

```rust
#[derive(Clone, PartialEq, Eq)]
struct PdfDocumentIdentity {
    path: PathBuf,
    password: Option<Box<str>>,
}
```

**`Job` に 1 つ持たせる** ([pdf_loader.rs:1783](../../src/pdf_loader.rs:1783))。
値は `execute()` ([pdf_loader.rs:2348](../../src/pdf_loader.rs:2348)) の中で
`decode_request` ([pdf_loader.rs:989](../../src/pdf_loader.rs:989)) を 1 回呼んで作る。
`render_request_collects_metrics` が既に同じことをしているので前例がある。
decode に失敗した要求は identity を持たない (= 常に open が必要とみなす)。

mtime / size は**含めない**。親は stat しないので持てない。ワーカー側の
`PdfDocumentCacheKey` ([pdf_loader.rs:395](../../src/pdf_loader.rs:395)) は 4 要素すべてで
判定し続ける (= 正しさはワーカーが持つ)。親の identity は**資源スケジューリング用**であって、
正しさの判定には使わない。

### 4.2 キュー状態

`JobQueue` ([pdf_loader.rs:1796](../../src/pdf_loader.rs:1796)) に足す:

- `worker_documents: Vec<Option<PdfDocumentIdentity>>` — worker_id 別の保持文書。
  長さは `in_flight_started_at` と同じ根拠 (起動時に設定された数) で確保する。
- `open_in_flight: usize` — `MSG_OPEN` を送っていて、まだ返っていない数。

更新規則:

| いつ | `worker_documents[worker_id]` |
| --- | --- |
| `MSG_OPEN` が成功応答 | `Some(job identity)` |
| `MSG_OPEN` がエラー応答 | `None` |
| 本要求の IPC が transport error (`send_recv_io` が `Err`) | `None` |
| 本要求が `STATUS_ERR` を返した | **変えない** (ワーカーは文書を保持したまま) |

### 4.3 判定は純関数にする

`non_critical_lane_caps` ([pdf_loader.rs:2661](../../src/pdf_loader.rs:2661)) と同じ形にする。
**実行時の待ち時間・queue 圧力・ストレージ種別に適応させない。**

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OpenAdmissionCaps {
    /// 同時に走ってよい open の総数。
    total: usize,
    /// そのうち背景 (HighNormal / Normal) が使ってよい上限。残りは Critical 用。
    non_critical: usize,
}
```

**値は固定 `total = 3`、`non_critical = 2`。設定項目にしない。**
利用者提案は「PDFium 並列度 10、同時 open は 3 まで」。ワーカー数は既に 3〜10 の設定が
あるので、ここにもう 1 つ数値の摘みを足すと説明が破綻する。**固定値で入れ、実測で
足りないと分かったときに設定化を検討する。**
ストレージ種別 (HDD / SSD) の自動判定は**しない** (「実行時状態で挙動を変えない」方針)。

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenAdmission { Admit, Defer }

fn decide_open_admission(
    priority: JobPriority,
    needs_open: bool,
    open_in_flight: usize,
    /// より高い優先度に、まだ開かれていない文書を待っている job があるか。
    higher_priority_open_pending: bool,
    caps: OpenAdmissionCaps,
) -> OpenAdmission;
```

規則:

1. `!needs_open` → **常に Admit**。再利用はディスクを使わない。
   (ページ送りの定常状態はここに落ちるので、既存の並列度は一切変わらない。)
2. `Critical` → `open_in_flight < caps.total` なら Admit。
3. `HighNormal` / `Normal` → `open_in_flight < caps.non_critical`
   **かつ** `!higher_priority_open_pending` なら Admit。

規則 3 の後半が §2.15。**時間窓ではなく、キューの中身から決まる。**

`higher_priority_open_pending` の定義 (これも純関数にする):

- `HighNormal` から見て: `critical` キューに、**どのワーカーも保持していない文書**の
  job が 1 件以上ある。
- `Normal` から見て: `critical` または `high_normal` キューに同じものがある。

「どのワーカーも保持していない」で判定するのは、保持しているワーカーが 1 つでもあれば
その job は open を必要とせずに始められるから。走査量は
(critical + high_normal の長さ) × (ワーカー数 ≦ 10) で、pop 1 回あたり。

### 4.4 pop

`try_pop_dispatch_job` ([pdf_loader.rs:2677](../../src/pdf_loader.rs:2677)) を
`worker_id` と open 判定を受け取る形に変える。

- レーンの優先順 (Critical → HighNormal → Normal) は変えない。
- **各レーンの中では、先頭から「このワーカーで今すぐ始められる最初の job」を取る。**
  先頭が open 待ちで塞がっているだけの理由で、後ろにある「このワーカーが既に保持している
  文書の job」を待たせない。これをやらないと、open 上限がそのまま render の上限になる。
- 既存の lane cap (`normal_in_flight` と `caps`) の判定は**そのまま残す**。
  open 許可はその上に重なる追加条件。
- pop に成功したとき、その job が open を要するなら `open_in_flight += 1` する。

### 4.5 dispatcher

`run_dispatcher` ([pdf_loader.rs:2714](../../src/pdf_loader.rs:2714)):

- pop 結果に「open が要るか」を持たせる。
- 既存の cancel / epoch skip 経路に来た場合、**取った open 枠を必ず返す**
  (skip は IPC を送らないので `MSG_OPEN` も送らない)。
- `MSG_OPEN` の往復が終わったら、成否にかかわらず `open_in_flight -= 1` して
  **`cv.notify_all()`** する (枠待ちで寝ている dispatcher を起こす)。
- 枠を待つために dispatcher が queue lock を持ったまま眠ってはならない。
  既存どおり「取れなければ `cv.wait`」で寝る。

### 4.6 ワーカー側

- `MSG_OPEN` を `decode_request` / `DecodedRequest` へ追加 (`path`, `password` のみ)。
- worker loop ([pdf_loader.rs:1180](../../src/pdf_loader.rs:1180) 付近) に腕を足し、
  `cache.with_document(path, password, |_doc, _key| Ok(()))` を呼んで
  `STATUS_OK` (payload なし) か `STATUS_ERR + message` を返す。
- **`MSG_OPEN` は既存のどの応答形式にも影響しない。** metrics フレームも出さない。

## 5. 計装

- `pool_dispatch` イベントに `needs_open` (bool) を足す。
- 新イベント `pdf/pool_open_request`: `open_ms`、`priority`、`pid`、`open_in_flight`
  (この要求を含む値)、`ok` (bool)。**perf 有効時のみ。**
- 新イベント `pdf/pool_open_deferred`: 許可枠が取れずに pop を見送った回数を
  レーン別・理由別 (`cap` / `higher_priority_pending`) に集計して**一定間隔で 1 行**。
  pop ごとに出すと静止時にログが膨らむ
  ([docs/idle-health-check.md](../idle-health-check.md) のゲートに引っかかる)。
  **既存の `pool_queue_snapshot` の 1 秒 tick に相乗りするのがいちばん安い。**
- **予測の検証**: 本要求が render metrics を持つとき、`worker.doc_reused` が
  親の想定 (`MSG_OPEN` を送った直後なので `true` のはず) と食い違ったら
  `pdf/pool_open_prediction_mismatch` を出す。**0 件が期待値**であり、出るなら
  モデルが間違っている。

## 6. やらないこと

- **ワーカー内部で待たせない。** ワーカーを塞ぐと、そのワーカーは再利用 job も処理できなくなる。
- **時間窓・sleep・retry で吸収しない。**
- **ストレージ種別を実行時に判定しない。**
- **設定項目を増やさない。**
- **`render_page_async` (in-process 経路) には触らない。** これはフルスクリーンの
  再レンダリング専用の現役経路で、プールを通らない (backlog §2.14 で別途扱う)。
- `PdfDocumentCache` の中身 (mtime/size を含む 4 要素判定) は変えない。

## 7. テスト

純関数はすべて表駆動で:

- `decide_open_admission` — 規則 1〜3 の全組み合わせ。特に
  「再利用は cap を無視して Admit」「Critical は `total` まで」
  「背景は `non_critical` まで、かつ上位に open 待ちがあれば Defer」。
- `higher_priority_open_pending` — 上位キューが空 / 上位に open 待ちあり /
  上位にあるがどれかのワーカーが保持している、の 3 系統。
- `try_pop_dispatch_job` — 既存テスト群 ([pdf_loader.rs:5050](../../src/pdf_loader.rs:5050)
  付近) に足す形で:
  - 保持文書と一致する job は、open 枠が満杯でも pop される
  - 先頭が open 待ちでも、後ろの再利用 job が pop される
  - 枠を返すと、直後の pop で待たされていた job が取れる
  - 既存の lane cap の挙動が変わっていない (**回帰**)
- `PdfDocumentIdentity` — `decode_request` の 4 種類すべてから同じ identity が出ること。
  `MSG_OPEN` の encode/decode round trip。
- `worker_documents` の更新規則 — §4.2 の表の 4 行を、状態遷移の純関数として。

**既存テストが赤くなったら止めて報告する。** 特に lane cap と epoch prune のテスト。

## 8. 期待される効果と、測って確かめること

- 5 並列 open → 3 並列になり、HDD 上の大きな PDF 群のカバー生成でシーク往復が減る。
- 前面 (Critical) の open が待っている間、背景の**新しい** open が始まらない。
  既に走っている open は止められない (PDFium の open は中断できない) ので、
  **待ちがゼロにはならない。効果は「積み上がらなくなる」こと。**
- ページ送りの定常状態 (同じ本) は再利用なので**一切変わらない**。

実機で `--perf-log` を取り、`worker_open_ms` の p99 / 最大と、
`ui/visible_thumb_all_ready` の変化を比べる。**SSD 上のフォルダでサムネイル生成が
遅くなっていないことも必ず確認する** (§3.1 の取引を避けたのが設計の主目的なので、
ここが悪化していたら設計が想定どおり動いていない)。
