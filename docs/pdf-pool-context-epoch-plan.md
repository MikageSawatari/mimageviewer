# PDF worker pool への context epoch 導入計画 (Codex round 4 反映版・実装 ready)

## 背景・問題

`%APPDATA%\mimageviewer\logs\perf_events.jsonl` の解析 (2026-05-23) で、
PDF を多数含むフォルダで Ctrl+↑↓ を高速に渡ると、サムネ表示が体感「数秒〜10 秒以上」
遅延するケースが計測された。

具体例:
- `e:\share\18\bookscan` (12 PDF が並ぶ親フォルダ) に入ったとき:
  - 最初の cover (page 0 @ 620 px) ready まで 1.3 秒 (=正常)
  - 2 番目以降 3.4 → 4.4 → 8.1 → 8.3 → **10.9 秒** と急激に悪化
- ユーザは入った直後に Ctrl+↓ で個別 PDF に入り、また bookscan に戻る、を高速に繰り返す
- 結果、bookscan の cover thumbnail が「画面に見えている範囲」と全くズレた順序で完成する

## 現状の構造

### 既存のキャンセル機構

`PdfWorkerPool` (`src/pdf_loader.rs`) は `Mutex<JobQueue> + Condvar` ベースで:
- `critical: VecDeque<Job>` (UI enumerate 用、1 worker 予約)
- `normal: VecDeque<Job>` (サムネ render 用、push_back / pop_front の FIFO)
- 各 `Job` は `cancel: Option<Arc<AtomicBool>>` を保持
- `run_dispatcher` は pop 直後に `cancel.load()` を確認し、true なら IPC せず Interrupted を返す
  (perf イベント `pool_cancel_queued`)
- `recv_timeout` ループ側 (requester) も 50 ms 間隔で cancel をチェックし、true なら早期 bail
  (perf イベント `pool_cancel_requester`)

`load_folder` → `start_loading_items` ([app.rs:6826](../src/app.rs)) が呼ばれると:
1. `App::cancel_token.store(true)` で旧トークンを倒す
2. `App::cancel_token` を新しい `Arc<AtomicBool>` に置き換え
3. `thumb_loader::bump_catchup_epoch()` で catchup queue (`pdf_meta` 背景書込) を一括クリア + cancel
4. reload_queue / heavy_io_queue / requested 等を `invalidate_idx_state_and_queues` で wipe

これで thumb worker 側はクリアされる。**しかし `PdfWorkerPool` の `normal` キューには
旧 cancel flag の付いたジョブが残ったまま**で、dispatcher が FIFO で順番に pop して
cancel を確認し、IPC を skip する。Normal cap は `worker_count - 1` ([pdf_loader.rs:1119](../src/pdf_loader.rs))
= 3-worker 環境では **2 worker** しか同時にサムネ render しないため、in-flight tail も最大 2 件分。

### なぜ「数秒」遅延するか (要因の分解)

| 要因 | 寄与 | 対策の射程 |
| --- | --- | --- |
| (A) in-flight IPC は止められない (PDFium 制約)。Normal cap=2 worker × 1.5 秒 ≒ 3 秒のテール | 数秒 | **本 PR の non-goal** (PDFium 自体に cancel API 無し) |
| (B) FIFO で次に走らせるべき "current context" のジョブが旧 context ジョブの後ろに並ぶ | 数百 ms〜2 秒 | **本 PR の主目的** |
| (C) `req.priority` (可視セル) フラグが PDF pool で常に `JobPriority::Normal` に潰される ([thumb_loader.rs:1699](../src/thumb_loader.rs)) | 数百 ms | **本 PR で追加対応** (Codex P2-6) |
| (D) Normal cap = `worker_count - 1` で 3-worker でも 2 並列にしかならない | 33% スループット低下 | 本 PR の non-goal (Critical 予約とのトレードオフ、別途検討) |

(B) と (C) を本 PR で潰す。(A)(D) は別 PR 候補。

### 既存の `LATEST_ENUMERATE_EPOCH`

`enumerate_pages_async` のみ 1 epoch 機構を持ち、ワーカーが pickup 時に最新と比較して
stale (= 古い enumerate) なら skip する。これは「PDF 連打して開いた PDF の最新だけ
列挙する」用。今回の問題は `render_page` 側にあり、こちらは epoch 機構を持たない。

## 設計方針

### 中核アイデア: render ジョブに **明示的な** context epoch を付ける

`PdfWorkerPool` の `Job` に `context_epoch: u64` を追加し、グローバル
`CURRENT_CONTEXT_EPOCH: AtomicU64` と比較して、古い epoch のジョブを
**(a) dispatcher pop 時に skip** および **(b) epoch bump 時にキュー一括 prune** する。

これにより、ユーザが新しいフォルダ/PDF に入った瞬間、pool の normal キューに残っている
旧コンテキストのジョブが即座に消える。in-flight IPC は止められないが、その後は
新コンテキストのジョブを取りに行く。

### Codex P1-1 対応: TOCTOU 防止のため epoch は **明示引数**

`render_page` 内で `current_render_context_epoch()` を読む案は **NG**。
理由:
- thumb worker が old folder の `req` を pop し、`req` から PDF render を呼ぶ直前に
  メインスレッドが bump → worker が拾った epoch は new、しかしこの render は old の
  ためのもの → stale 化されず無駄レンダリング、もしくは prune を逃れる。
- 対策: **UI スレッドで enqueue する瞬間に `LoadRequest.context_epoch` を焼き付け、
  worker → render_page → pool.execute と渡す**。`render_page` は引数で受け取るだけ。

### Codex P1-2 対応: epoch=0 を明示 API として確立

非 UI 経路 (background cache creation、`process_meta_only`、neighbor prefetch ……) は
UI ナビゲーションで stale 化させてはいけない (Codex P2 の Round 1 で既に確認済み)。
→ これらは `render_page(.., context_epoch=0, ..)` で呼ぶ。

epoch=0 は予約値 (= epoch チェックを無効化する sentinel)。`bump_render_context_epoch` は
1 から始めて単調増加する。

### Codex P1-3 / round 2 P1-2 対応: Interrupted の取り扱いは PDF render 経路に限定

`load_one_cached` ([thumb_loader.rs:1820](../src/thumb_loader.rs)) で失敗時の処理:
- 現状: `cancel.load() == true` のときだけ silent return (Failed 化を避ける)。
  それ以外の Err は `ThumbMsg { image: None, canceled: false }` を送り、UI 側は Failed 化。
- epoch prune で **`io::Error::Interrupted` が** `image::ImageError::IoError(io)` **に
  ラップされて返る** ([thumb_loader.rs:1735](../src/thumb_loader.rs) で wrap)。
- Codex round 2 指摘: 単に `e.kind() == Interrupted` だと:
  (a) コンパイルが通らない (e は `image::ImageError`)
  (b) Susie / image crate 内部の Interrupted まで巻き込む

**修正案**: PDF render 経路でだけ Interrupted を「stale/canceled」相当として silent 化する。
具体的には:

1. `render_page` の戻り値型は変えず、**PDF 由来の Interrupted は `image::ImageError::IoError`
   で wrap される従来形のまま**。
2. `load_one_cached` の Err ハンドラで、**PDF page リクエストの場合のみ** 内側 io_err を
   抽出して kind チェックする:
```rust
let pdf_interrupted = pdf_page.is_some()
    && matches!(
        &e,
        image::ImageError::IoError(io) if matches!(io.kind(), std::io::ErrorKind::Interrupted)
    );
let cancelled = cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));
if cancelled || pdf_interrupted {
    // silent return + gen_done.fetch_add
    return;
}
// それ以外は従来の Failed 通知
```
3. Susie / image crate の Interrupted (= 通常起こらないが) は従来通り Failed 通知のままにする。

`process_meta_only` / `process_neighbor_prefetch` 等の background 経路は epoch=0 で呼ぶため
epoch 由来の Interrupted は来ない。cancel.load() 経由のみ。改修不要。

### Codex P2-4 / round 2 P1-1 対応: bump 位置は **共通 helper + 2 箇所** から呼ぶ

`start_loading_items` ([app.rs:6826](../src/app.rs)) は通常フォルダ / ZIP / PDF as folder /
favsearch ([app.rs:5729](../src/app.rs)) を全てカバーするが、**Codex round 2 指摘**:
`replace_search_view_items` ([global_search_ui.rs:1008,1062](../src/global_search_ui.rs))
は `install_new_items` を直接呼んで `start_loading_items` をバイパスするため、
Ctrl+G 結果差し替え経路で epoch bump が漏れる。

**修正案**: items 差し替えの共通ヘルパを作る。

**Codex round 4 P1**: `start_loading_items` と `replace_search_view_items` は
ライフサイクルが異なる:
- `start_loading_items` は worker 再 spawn ([app.rs:7176](../src/app.rs)) を行う → cancel_token を flip して OK
- `replace_search_view_items` は worker 再 spawn しない (queue invalidate のみ
  [global_search_ui.rs:1062, 1070](../src/global_search_ui.rs)) → cancel_token を flip
  すると **既存 worker が exit して消費者不在になる**

→ **helper を 2 つに分ける**:

```rust
// app.rs (private fn)
impl App {
    /// `start_loading_items` 用: フル bump (cancel_token + wake + 新 token + catchup + render_epoch)。
    /// 後段で worker を再 spawn するので、cancel_token を flip しても問題ない。
    fn bump_full_context_for_load(&mut self) {
        self.cancel_token.store(true, Ordering::Relaxed);
        self.wake_all_workers();   // 既存 start_loading_items 挙動を維持
        self.cancel_token = Arc::new(AtomicBool::new(false));
        crate::thumb_loader::bump_catchup_epoch();
        let _ = crate::pdf_loader::bump_render_context_epoch();
    }

    /// `replace_search_view_items` 用: render epoch のみ bump。
    /// cancel_token は **touch しない** (worker が exit してしまうため)。
    /// catchup epoch も触らない (Ctrl+G は PDF 階層を変えないので catchup の
    /// neighbor prefetch は引き続き有効)。
    ///
    /// **Codex round 5 P1**: `global_search_ui.rs` から呼ぶので `pub(crate)` で公開。
    pub(crate) fn bump_render_epoch_only(&mut self) {
        let _ = crate::pdf_loader::bump_render_context_epoch();
    }
}
```

呼び出し箇所:
- `start_loading_items` ([app.rs:6826 付近](../src/app.rs)): 既存の cancel_token 操作と
  `bump_catchup_epoch` 呼び出しを `bump_full_context_for_load()` に置換
- `replace_search_view_items` ([global_search_ui.rs:1062 付近](../src/global_search_ui.rs)):
  `install_new_items` 呼び出し直前に `bump_render_epoch_only()` を追加。**既存の worker /
  cancel_token は維持**。
- `load_folder` ([app.rs:4389](../src/app.rs)) の `bump_catchup_epoch` 直呼びは削除
  (`start_loading_items` 経由に統合)

経路カバレッジの最終確認は実装時に grep で:
- `install_new_items` の全呼び出し元を列挙
- 各箇所で `bump_render_context()` が直前に呼ばれているか確認
- 通っていない経路は bypass バグなので個別に判断 (helper を呼ぶ or items_generation の
  bump だけで十分か)

### Codex P2-6 対応: `req.priority` を PDF pool に伝える

現状: `process_load_request` → `load_one_cached` → `render_page` の経路で、
`render_page(.., JobPriority::Normal, ..)` を **常にハードコード** ([thumb_loader.rs:1699](../src/thumb_loader.rs))。

UI スレッドが立てた `req.priority: bool` (true = 画面に見えている可視セル) は無視される。
これは前回のチャットで観測した「visible 0-19 と hidden 20-31 の順序が正しく出ない」事象の
主因の 1 つ。

→ **`render_page` 呼び出し時に `req.priority == true` なら `JobPriority::Critical`** に
変える案 …は **採用しない**。理由:
- Critical は **1 worker 専有予約** ([pdf_loader.rs:1085](../src/pdf_loader.rs)) のため
  本来 enumerate のみが使う想定。サムネ render が大量に Critical を取ると enumerate
  (UI nav が呼ぶ) を阻害する。
- そもそも Critical は「数千 ms 級の他ジョブを抜いて即実行」というセマンティクス。
  visible cell の優先度はもっと弱い (= 同じ Normal 内で先に取られればよい)。

→ 代わりに **`JobPriority` を 3 段階に拡張**:
```rust
enum JobPriority {
    Critical,   // UI nav の enumerate (1 worker 予約は維持)
    HighNormal, // 可視セルのサムネ render (新規)
    Normal,     // 先読み・background catch-up
}
```

dispatcher の pop 順: `critical → high_normal → normal`。Normal cap は high_normal +
normal の合計に適用。3 worker の場合 max 2 件まで HighNormal/Normal の合算で in-flight。

これで visible/hidden の優先順位が PDF pool 内でも保たれる。

epoch との関係: HighNormal / Normal 両方 epoch チェック対象 (= stale prune 対象)。
Critical は対象外 (UI nav の直結だから当然 fresh)。

### Codex P2-5 対応: 根本原因の精度

stale prune を入れても 2-worker × 1.5 秒の in-flight tail は残る。
→ 本 PR の **目標は "queue accumulation を解消する" まで**。残る in-flight tail は
   別 PR でPDFium のレンダリングストリーム化等を検討する (= Non-goal、ドキュメント明記)。

期待値:
- bookscan の cover 最遅 ready: 11 秒 → **~2 秒** (Normal cap 2 × ~1 秒/cover × 6 ジョブ)
- 「visible 範囲のサムネが先に埋まる」感: PDF page-grid 内でも改善

## 実装計画

### Phase 1: PDF pool 側 (`src/pdf_loader.rs`)

#### 1.1 `JobPriority` 3 段階化

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobPriority {
    Critical,
    HighNormal,
    Normal,
}
```

`JobQueue` 構造体:
```rust
struct JobQueue {
    critical: VecDeque<Job>,
    high_normal: VecDeque<Job>,   // NEW
    normal: VecDeque<Job>,
    /// HighNormal + Normal の合算 in-flight (= 旧 normal_in_flight)
    normal_in_flight: usize,
    workers_busy: usize,
    shutdown: bool,
}
```

#### 1.2 `Job` に `context_epoch: u64` を追加

```rust
struct Job {
    request: Vec<u8>,
    cancel: Option<Arc<AtomicBool>>,
    reply: mpsc::Sender<...>,
    priority: JobPriority,
    enqueued_at: Instant,
    perf_key: Option<String>,
    /// enqueue 時点の context epoch。CURRENT_CONTEXT_EPOCH より小さければ stale。
    /// 0 は「epoch チェック対象外」(= background 経路用 sentinel)。
    context_epoch: u64,
}
```

#### 1.3 グローバル epoch + public API

```rust
/// Render ジョブの「コンテキスト世代」。`start_loading_items` で bump。
/// 0 は予約 (epoch チェック無効化)。
static CURRENT_CONTEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

pub fn bump_render_context_epoch() -> u64 {
    let new = CURRENT_CONTEXT_EPOCH.fetch_add(1, Ordering::Relaxed) + 1;
    if let Some(pool) = POOL.get() {
        pool.prune_stale_jobs(new);
    }
    new
}

pub fn current_render_context_epoch() -> u64 {
    CURRENT_CONTEXT_EPOCH.load(Ordering::Relaxed)
}
```

#### 1.4 `PdfWorkerPool::execute` シグネチャ拡張

```rust
fn execute(
    &self,
    request: &[u8],
    cancel: Option<&Arc<AtomicBool>>,
    priority: JobPriority,
    perf_key: Option<String>,
    context_epoch: u64,   // NEW: 0 で epoch チェック無効
) -> std::io::Result<Vec<u8>>
```

#### 1.5 `PdfWorkerPool::prune_stale_jobs`

```rust
fn prune_stale_jobs(&self, current_epoch: u64) {
    let drained: Vec<Job> = {
        let (mtx, _cv) = &*self.queue;
        let mut q = mtx.lock().unwrap();
        let mut dropped = Vec::new();
        // HighNormal と Normal の両方をプルーン (Critical は touch しない)
        for queue in [&mut q.high_normal, &mut q.normal] {
            let kept: VecDeque<Job> = queue
                .drain(..)
                .filter_map(|j| {
                    if j.context_epoch != 0 && j.context_epoch < current_epoch {
                        dropped.push(j);
                        None
                    } else {
                        Some(j)
                    }
                })
                .collect();
            *queue = kept;
        }
        dropped
    };

    // Mutex 外で reply を送る
    let count = drained.len();
    for j in drained {
        if crate::perf::is_enabled() {
            let waited_ms = j.enqueued_at.elapsed().as_secs_f64() * 1000.0;
            crate::perf::event(
                "pdf",
                "pool_prune_stale_epoch",
                j.perf_key.as_deref(),
                0,
                &[
                    ("waited_ms", serde_json::Value::from(waited_ms)),
                    ("job_epoch", serde_json::Value::from(j.context_epoch)),
                    ("current_epoch", serde_json::Value::from(current_epoch)),
                    ("priority", serde_json::Value::from(format!("{:?}", j.priority))),
                ],
            );
        }
        let _ = j.reply.send(Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "context epoch advanced",
        )));
    }
    if count > 0 {
        crate::logger::log(format!(
            "pdf-pool: pruned {count} stale jobs (current_epoch={current_epoch})"
        ));
    }
}
```

#### 1.6 `run_dispatcher` の pop ロジック + epoch チェック

pop の優先順位を `critical > high_normal > normal` に変更:
```rust
let job = {
    let mut q = mtx.lock().unwrap();
    loop {
        if q.shutdown { break None; }
        if let Some(j) = q.critical.pop_front() {
            q.workers_busy = q.workers_busy.saturating_add(1);
            break Some(j);
        }
        let max_n = if critical_reservation_active() {
            worker_count.saturating_sub(1).max(1)
        } else {
            worker_count.max(1)
        };
        if q.normal_in_flight < max_n {
            // HighNormal を先に
            if let Some(j) = q.high_normal.pop_front() {
                q.normal_in_flight += 1;
                q.workers_busy = q.workers_busy.saturating_add(1);
                break Some(j);
            }
            if let Some(j) = q.normal.pop_front() {
                q.normal_in_flight += 1;
                q.workers_busy = q.workers_busy.saturating_add(1);
                break Some(j);
            }
        }
        q = cv.wait(q).unwrap();
    }
};
```

pop 後の cancel + epoch チェック (両方とも `Err(Interrupted)` を返す):
```rust
let cancelled = job.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));
let current = CURRENT_CONTEXT_EPOCH.load(Ordering::Relaxed);
let stale_epoch = job.context_epoch != 0 && job.context_epoch < current;

if cancelled || stale_epoch {
    let kind = if cancelled { "pool_cancel_queued" } else { "pool_stale_epoch_skip" };
    // perf event
    let _ = job.reply.send(Err(io::Error::new(io::ErrorKind::Interrupted, ...)));
} else {
    // IPC 実行
}
```

#### 1.7 `JobPriority` の判定 (Normal 完了時の `normal_in_flight` 減算)

```rust
let counts_against_normal = matches!(
    job.priority,
    JobPriority::HighNormal | JobPriority::Normal
);
// ... IPC または skip ...
if counts_against_normal {
    q.normal_in_flight = q.normal_in_flight.saturating_sub(1);
}
```

### Phase 2: render_page 経路への配線

#### 2.1 `render_page` シグネチャ拡張

```rust
pub fn render_page(
    pdf_path: &Path,
    page_num: u32,
    target_px: u32,
    password: Option<&str>,
    cancel: Option<Arc<AtomicBool>>,
    priority: JobPriority,
    context_epoch: u64,   // NEW
) -> std::io::Result<RenderResult>
```

呼び元 (= **Codex round 2 で網羅確認**、全 6 箇所):

| ファイル:行 | 呼び元 | priority | context_epoch |
| --- | --- | --- | --- |
| `thumb_loader.rs:1693` | `load_one_cached` (UI thumb worker) | `req.priority ? HighNormal : Normal` | `req.context_epoch` |
| `thumb_loader.rs:1290` | `process_neighbor_prefetch` (CatchupQueue) | `Normal` | `0` (background) |
| `thumb_loader.rs:2242` | `build_and_save_one_pdf` (batch cache 生成) | `Normal` | `0` (background) |
| `app.rs:13517` | フルスクリーン (current=Critical / prefetch=Normal、`start_fs_load` 内で分岐 [app.rs:13411](../src/app.rs)) | 既存通り維持 | **current=`0` / prefetch=`current_render_context_epoch()`** |
| `app.rs:17425` | bulk cache creator | `Normal` | `0` (background) |
| `app.rs:17486` | bulk cache creator (別ジョブ種) | `Normal` | `0` (background) |

**フルスクリーンの扱い** (`app.rs:13517`、**Codex round 3 P2-2 反映**):
Critical 現在ページは epoch=0 (= epoch チェック対象外、`fs_pending.cancel` で制御)。
**Normal 先読みは current epoch を使う** — フォルダ移動で stale 化したら pool 側で
prune される方が cleanup が早い。`fs_pending.cancel` は functional には十分だが、
epoch prune の方が in-queue ジョブを即解放できるので並列性が改善する。
`start_fs_load` 内で priority に応じて使い分け:
```rust
let context_epoch = if priority == JobPriority::Critical {
    0
} else {
    crate::pdf_loader::current_render_context_epoch()
};
```

**bench `src/bin/bench_scroll.rs:293`**: `LoadRequest::default()` ベースで構築。
`context_epoch: 0` で問題なし (= batch なので background 扱い)。

#### 2.2 `LoadRequest` に `context_epoch: u64` フィールドを追加

`src/thumb_loader.rs` の `LoadRequest`:
```rust
pub struct LoadRequest {
    // ... 既存 ...
    /// PDF render が pool に流れるときに焼き付ける context epoch。
    /// 0 = epoch チェック対象外 (= background 経路)。
    pub context_epoch: u64,
}
```

UI スレッドで enqueue する箇所 ([app.rs:9743 等](../src/app.rs)):
```rust
req.context_epoch = crate::pdf_loader::current_render_context_epoch();
```
を `req.items_gen = self.items_generation` の隣に追加。

**Codex round 2 で網羅確認**:

| ファイル:行 | enqueue 箇所 | 設定すべき context_epoch |
| --- | --- | --- |
| `app.rs:9735, 9743` | 通常 enqueue (`make_load_request` 周辺) | `current_render_context_epoch()` |
| `app.rs:10048` | idle quality upgrade enqueue | `current_render_context_epoch()` **要追加** (Codex round 2 指摘) |
| `app.rs:19673, 19706` | `apply_folder_thumb_pin` の `LoadRequest` リテラル直書き | `0` または `current_render_context_epoch()` (pin 解決は通常 UI navigation 経由なので current で OK)。**フィールド追加で coverage 強制 (= リテラルがコンパイルエラーになる)** |
| `CatchupQueue` の enqueue (NeighborPrefetch / MetaOnly) | thumb_loader 内 | `0` (background) |
| Bulk cache creator | app.rs / cache_creator | `0` (background) |

**Default 値**: `LoadRequest::default()` は `context_epoch: 0` にする (= background 想定)。
UI 経路で明示的にセットしなかった場合に "epoch チェック対象外" として扱われる。
通常 enqueue 箇所は全て明示セットを義務化、grep で確認。

#### 2.3 worker thread での propagation

`process_load_request` ([thumb_loader.rs:556](../src/thumb_loader.rs)) → `load_one_cached`
→ `render_page` まで `req.context_epoch` を引数で渡す (新フィールド追加)。

`load_one_cached` 内の `render_page` 呼び出し ([thumb_loader.rs:1699](../src/thumb_loader.rs)):
```rust
let priority = if req.priority {
    crate::pdf_loader::JobPriority::HighNormal
} else {
    crate::pdf_loader::JobPriority::Normal
};
crate::pdf_loader::render_page(
    path,
    pg,
    target_px,
    password,
    cancel.cloned(),
    priority,
    context_epoch,
)
```

`load_one_cached` のシグネチャに `context_epoch: u64` を追加 (呼び元は req から渡す)。

### Phase 3: Interrupted の取り扱い

`load_one_cached` の render Err ハンドラ ([thumb_loader.rs:1820](../src/thumb_loader.rs)) を修正。
**Codex round 3 P1-1**: render_page の Err は `image::ImageError::IoError(io::Error)` で
ラップされている ([thumb_loader.rs:1735](../src/thumb_loader.rs))。`io.kind()` をそこから
取り出す必要がある。

```rust
Err(e) => {
    // PDF render 経路の Interrupted は epoch prune / dispatcher cancel の合図。
    // それ以外 (Susie / image crate / WIC) の Interrupted は本物の異常なので Failed 化する。
    let pdf_interrupted = pdf_page.is_some()
        && matches!(
            &e,
            image::ImageError::IoError(io) if io.kind() == std::io::ErrorKind::Interrupted
        );
    let cancelled = cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));
    if cancelled || pdf_interrupted {
        crate::logger::log(format!(
            "    idx={idx:>4} cancelled/pdf-interrupted  {display_name}"
        ));
        gen_done.fetch_add(1, Ordering::Relaxed);
        return;
    }
    crate::logger::log(format!("    idx={idx:>4} FAIL {e}  {display_name}"));
    // ... 既存の Failed 通知 ...
}
```

ガード条件:
- `pdf_page.is_some()` を必ず付ける → Susie / ZIP / 通常画像の Interrupted を巻き込まない
- `image::ImageError::IoError(io)` パターンマッチで内側 io.kind() を見る (compile 通す)
- Codex round 3 確認: PDFium crash は `ErrorKind::Other` 系で返るので `Interrupted` を
  silent 化しても本物の crash を隠さない

`process_meta_only` ([thumb_loader.rs:1186](../src/thumb_loader.rs))、
`process_neighbor_prefetch` ([thumb_loader.rs:1290](../src/thumb_loader.rs))、
bulk cache creator ([app.rs:17425, 17486, thumb_loader.rs:2242](../src/app.rs)) は
background 経路で epoch=0 を使うので epoch prune 対象外 → Interrupted は従来通り
cancel.load() 経由のみ。改修不要 (確認のみ、本 PR スコープ外)。

### Phase 4: App 側からの bump 配線

#### 4.1 `start_loading_items` で full bump

`src/app.rs:6826` 付近の既存コード:
```rust
fn start_loading_items(&mut self, items: Vec<GridItem>, ...) {
    // ... existing ...
    self.cancel_token.store(true, Ordering::Relaxed);
    self.wake_all_workers();
    let cancel = Arc::new(AtomicBool::new(false));
    self.cancel_token = Arc::clone(&cancel);
    crate::thumb_loader::bump_catchup_epoch();
    // ...
}
```
を以下に置換:
```rust
fn start_loading_items(&mut self, items: Vec<GridItem>, ...) {
    // ... existing ...
    self.bump_full_context_for_load();   // cancel + wake + new token + catchup + render_epoch
    let cancel = Arc::clone(&self.cancel_token);  // helper が生成した新 token を取得
    // ...
}
```

#### 4.2 `replace_search_view_items` に render epoch のみの bump

`src/global_search_ui.rs:1062` 付近、`install_new_items` 呼び出し直前に:
```rust
self.bump_render_epoch_only();   // PDF pool の stale ジョブを drop
self.install_new_items(new_items);
```

#### 4.3 `load_folder` 内の `bump_catchup_epoch` を削除

`app.rs:4389` の `bump_catchup_epoch` 呼び出しを削除し、`start_loading_items` に
一元化 (helper 内で呼ばれる)。理由: `load_pdf_as_folder` 直呼び (password dialog
経路) で抜けるのを防ぐため。

確認事項: `install_new_items` の全呼び出し元を grep で網羅:
- `start_loading_items` ([app.rs:6882](../src/app.rs))
- `replace_search_view_items` ([global_search_ui.rs:1062](../src/global_search_ui.rs))
- 他はテストコードのみ (Codex round 4 で確認済み)

通常 install 経路は上記 2 つで網羅。新規経路を追加した場合は同じヘルパを呼ぶこと。

### Phase 5: テスト

#### 5.1 ユニットテスト (新規 `src/pdf_loader.rs` 内 `#[cfg(test)] mod`)

- `bump_render_context_epoch` を呼んだら `current_render_context_epoch` が +1
- `prune_stale_jobs(current)` で `context_epoch < current` の HighNormal/Normal が drop、
  reply に Interrupted が届く
- Critical はプルーンされない
- `context_epoch == 0` はプルーン対象外
- **race ケース** (Codex P3): 古い epoch を取得した後 bump、enqueue 後に dispatcher が
  pop で stale 検出して skip すること
- 同じ folder reload (= 連続 bump) で順次正しく epoch が進むこと
- HighNormal が Normal より先に pop されること
- HighNormal + Normal の合計が `normal_in_flight` cap を超えないこと

#### 5.2 統合テスト (手動 + perf log)

`scripts/perf_smoke.sh` 走行手順:
1. `--perf-log` 付きで mIV を起動
2. bookscan 相当の PDF 多数フォルダに移動
3. Ctrl+↑↓ を 5-10 秒連打
4. `python scripts/analyze_perf.py <log> hitches` で 50ms 超ヒッチが減ったか確認
5. `pool_stale_epoch_skip` イベント数を count

#### 5.3 perf 計装 (新規)

- `pdf/pool_stale_epoch_skip` — dispatcher pop で epoch stale を検出
- `pdf/pool_prune_stale_epoch` — bump 時の一括 prune

既存 `pool_cancel_queued` は減ると予想 (= 多くが prune 経由に移る)。

### Phase 6: ドキュメント更新

#### `docs/async-architecture.md`
- §1 のワーカー一覧 PdfWorkerPool 行を更新 (3 段階優先度 + epoch チェック)
- §2.3 ワーカーキュー: `pdf_pool.queue` 行に context_epoch / prune_stale / HighNormal の説明を追加
- §3 キャンセル規約: 新規 3.6 として `PdfWorkerPool` の epoch ベース stale 検出 + 3 段階優先度

#### `CLAUDE.md`
- 「PDF ワーカープール」セクションに epoch ベースのキャンセル機構を追記

#### `docs/pdf-issues.md` または本ファイル
- 調査経緯 + 修正方針を残す。実装後に「実装メモ」セクションを追加。

## 影響範囲とリスク

| 領域 | 影響 |
| --- | --- |
| `src/pdf_loader.rs` | JobPriority enum / Job / JobQueue / PdfWorkerPool / run_dispatcher / render_page / public API 追加 |
| `src/thumb_loader.rs` | LoadRequest / process_load_request / load_one_cached / Interrupted ハンドリング |
| `src/app.rs` | start_loading_items / load_folder (bump 削除) / 各種 enqueue 箇所での context_epoch セット |
| `src/ui_dialogs/cache_creator.rs` 等 | render_page 直呼び箇所に epoch=0 引数追加 |
| `docs/async-architecture.md` | 設計反映 |

### リスク

| リスク | 対策 |
| --- | --- |
| TOCTOU (Codex P1-1) | LoadRequest に焼き付け、render_page は引数で受ける |
| Background ジョブ巻き込み (Codex P1-2) | 明示 context_epoch、background は 0 |
| Failed 化 (Codex P1-3) | Interrupted を cancel と同等扱い |
| bump 漏れ (Codex P2-4) | start_loading_items に集約、grep で経路網羅確認 |
| HighNormal が enumerate(Critical) を阻害 | Critical 予約は維持 (1 worker)、HighNormal は Normal 枠内で先取り |
| prune と pop-time check の二重カウント | prune は perf event 1 個だけ emit、pop-time check は別 event 名で emit |
| race (epoch 取得 → bump → 自分の enqueue が prune 後に来る、Codex P3) | pop-time check で必ず stale を捕捉 (= prune が漏らしても dispatcher が拾う) |

### Non-goals

- in-flight IPC のキャンセル (PDFium 制約、別 PR で検討)
- Normal cap = `worker_count - 1` の見直し (Critical 予約とのトレードオフ、別 PR)
- enumerate_pages_async の epoch 機構統合 (既存実装で十分機能)
- per-PDF / per-folder の細かい epoch 粒度 (将来必要になれば)

## 期待効果

perf log 現状値 vs 期待値:

| 指標 | 現状 | 期待 |
| --- | --- | --- |
| bookscan の cover 最遅 ready | ~11 秒 | ~2 秒 |
| PDF page-grid 内の visible 0..19 と hidden 20..31 の混在 | あり | ほぼ無し (HighNormal 先取り) |
| Ctrl+↑↓ 連打時 `pool_cancel_queued` 件数 | 367 / 98 秒 | 大幅減 (大半が `pool_prune_stale_epoch` に移行) |
| `pool_stale_epoch_skip` (新規) | N/A | バースト発生 (race 対応分のみ) |
| `pool_prune_stale_epoch` (新規) | N/A | 各 bump で数十件レベル |
| アクティブな PDF の page-0 ready 時間 (Critical 経由) | ~200 ms | 変化なし |

## 実装順序

1. **Phase 1.1-1.3**: `JobPriority` 3 段階化 + `Job.context_epoch` + 公開 API。既存呼び元は
   全て (priority, epoch=0) で呼ぶよう更新 → ビルド通す
2. **Phase 1.5-1.7**: `prune_stale_jobs` + dispatcher の優先順位変更 + epoch チェック
3. **Phase 2**: `LoadRequest.context_epoch` + worker propagation + `render_page` シグネチャ
4. **Phase 3**: Interrupted ハンドリング修正
5. **Phase 4**: `start_loading_items` に bump 集約、`load_folder` から削除
6. **Phase 5.1**: ユニットテスト
7. **Phase 5.2**: 手動 perf 検証 (bookscan で再現)
8. **Phase 6**: ドキュメント更新

各ステップ単独で `cargo build` + 既存テスト pass を確認、最後に手動検証。
