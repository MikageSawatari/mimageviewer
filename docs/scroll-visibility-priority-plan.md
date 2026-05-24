# スクロール後の可視サムネ優先化 + 計装拡張 (Codex R1 反映版)

## 背景・問題

`HarvestOnCancel` 導入 (commit 5c2a3526) 後の perf log 解析 (2026-05-24 セッション) で、
ユーザが Down キー連打で 444 idx を 2 秒で移動した後、停止位置の最初のサムネが
表示されるまで **3.3 秒** 待たされる現象を確認。

### 計測結果

- bookscan に BS で復帰 (t=12.93s)、idx 0-19 が即 cache hit で表示
- 連続 Down キー: sel=47 → 491 を t=13.5-15.5s (2 秒、444 idx)
- 停止後、最初の thumb/ready: **t=18.78s (idx=480)** ← 停止から 3.3 秒

### 根本原因 (Codex R1 で精緻化)

`req.priority` は `LoadRequest` enqueue 時点で評価されるが、**`reload_queue` /
`heavy_io_queue` は `update_keep_range_and_requests` で毎フレーム re-tag されている**
([app.rs:9824, 9851](../src/app.rs)):

```rust
for r in q.iter_mut() {
    r.priority = r.idx >= visible_raw_start && r.idx < visible_raw_end;
}
```

つまり thumb worker の reload_queue 内ではスクロール追従できている。

**問題は PDF pool 側**:

1. t=13.5s: 可視範囲 idx 0-19、prefetch で idx 100-115 を priority=False で reload_queue へ
2. 直後フレーム: thumb worker が idx 100 を pop → render_page() → **pdf_pool.normal に積まれる
   (LoadRequest.priority=false → JobPriority::Normal で `normal` VecDeque)**
3. ユーザがどんどんスクロール → 可視範囲が idx 100 に到達
4. 次フレーム: reload_queue は re-tag されるが、**idx 100 は既に reload_queue に居らず
   pdf_pool.normal に居る**。pool 側は priority を再評価する仕組みが無い
5. pool の Normal lane で他の Normal ジョブと FIFO 公平処理 → 数百 ms〜数秒待ち
6. 結果として「停止 → 1 枚目 3.3 秒待ち」

## 設計方針

2 段階で進める:

1. **計装拡張**: 「スクロール後の可視サムネタイミング」を客観値化
2. **修正**: pdf_pool に `promote_to_high_normal` を追加、visible 範囲変更時に呼ぶ

両方を 1 PR で入れる理由:
- 修正の効果を新計装で即計測できる
- before/after が再現可能になる

## Phase 1: 計装拡張

### 1.1 新規 perf イベント

#### `ui/scroll_settle` — スクロール停止検出
スクロール (key/wheel) から **300 ms** 以上経過、かつ前 settle 以降に scroll が発生
した場合のみ emit。

**Codex R1 P1-2 / R2 P1 対応**: 検出ロジックは **`render_grid` の後** に走らせる必要が
ある。理由: `render_grid` ([app.rs:18675](../src/app.rs)) が `scroll_to_selected` と
scrollbar dragback を反映するため、それより前で settle 判定すると 1 フレーム古い
状態を見る。

具体的には [App::update](../src/app.rs:18079) の `render_grid` 完了後に:
- `scroll_offset_y` の前回値と比較、変化があれば `last_scroll_event_at = Some(Instant::now())`
- settle 判定 (= `last_scroll_event_at` から 300ms 経過 + 同 visible_set で未 emit) は
  **次フレーム** の同じ位置で行う (= 反映済み visible_set を使える)

```json
{
  "cat": "ui",
  "kind": "scroll_settle",
  "tid": 1,
  "visible_set_size": 16,
  "visible_first_idx": 480,
  "visible_last_idx": 511,
  "pending_visible": {"loaded": 1, "pending": 15, "evicted": 0, "failed": 0},
  "requested_in_visible": 12,
  "queue_state": {
    "pdf_pool": {"critical": 0, "high_normal": 3, "normal": 48, "in_flight": 3},
    "reload_queue": 42,
    "heavy_io_queue": 0
  }
}
```

**Codex R1 P3-1 対応**: `visible_set` のフィンガープリント (= ハッシュ) で dedupe。
同じ visible_set で連続して settle しない。

#### `ui/visible_thumb_first_ready` / `visible_thumb_all_ready`
最後の `scroll_settle` 以降で、可視範囲のサムネが Loaded 化したタイミング。

**Codex R1 残課題対応**:
- settle 時点で既に Loaded だったアイテムは latency=0 でカウント (= 「既に出てる」分)
- `first_ready` は settle 後に NEW で Loaded 化した最初の可視 idx
- `all_ready` は visible_set 全件が Loaded (or 諦め: Failed / Evicted) になった瞬間

```json
{
  "cat": "ui",
  "kind": "visible_thumb_first_ready",
  "tid": 1,
  "settle_seq": 42,
  "latency_ms": 3340,
  "idx": 480,
  "already_loaded_at_settle": 1,
  "target_count": 16
}
```

#### `pdf/pool_queue_snapshot` — 定期 emit
1 秒に 1 回、pdf_pool の queue state snapshot。

**Codex R1 P2 / R2 P2 / R3 P2 対応**:
- `POOL.get()` で pool 未初期化なら **skip emit** (= 無 PDF のフォルダで pool 起動しない)
- in_flight metadata: `JobQueue` に `in_flight_started_at: Vec<Option<Instant>>` を追加。
  **`POOL_SIZE` (= 3、定数) サイズで `vec![None; POOL_SIZE]` で固定確保**。
  `run_dispatcher` の worker_id を index に使う。`pending_workers.push((i, ...))` で
  原本の loop index `i` を保持するので、worker 0 が spawn 失敗で worker 1, 2 だけ動いても
  index は POOL_SIZE 内に収まる (= 安全)
- `run_dispatcher` で IPC 実行直前に `slot[worker_id] = Some(now)`、完了直後に `slot[worker_id] = None`
- `in_flight_age_ms` (max/p95/p50) は Vec の `Some(t)` を集めて `Instant::now() - t` から計算

emit トリガは frame-driven: App に `last_pdf_pool_snapshot_at: Instant` を持ち、
`elapsed() >= Duration::from_secs(1)` で emit + reset。frame jitter は許容。

```json
{
  "cat": "pdf",
  "kind": "pool_queue_snapshot",
  "tid": 1,
  "critical": 0,
  "high_normal": 5,
  "normal": 32,
  "in_flight": 3,
  "in_flight_age_ms": {"max": 1240, "p95": 980, "p50": 410}
}
```

### 1.2 既存イベント拡張

#### `thumb/enqueue` フィールド分割 (Codex R1 P2-6 対応)

旧案の `reason` は visibility / source / cache の 3 軸混在で不適切。**3 つの独立フィールド**に分ける:

```json
{
  "cat": "thumb",
  "kind": "enqueue",
  "idx": 480,
  "priority": false,
  "visibility": "prefetch_forward",  // visible / prefetch_forward / prefetch_backward
  "source": "normal",                 // normal / folder_pin / idle_upgrade
  "skip_cache": false,
  "queue": "regular"
}
```

`visibility`: `i ∈ visible_set` なら `"visible"`、`i > visible_end` なら `"prefetch_forward"`、
`i < visible_start` なら `"prefetch_backward"`。
`source`: `apply_folder_thumb_pin` 経由なら `"folder_pin"`、idle upgrade なら `"idle_upgrade"`、
それ以外 `"normal"`。

#### `input/grid_key` / `input/grid_wheel` 拡張
`visible_first_idx`, `visible_last_idx`, `visible_set_size` を追加。

### 1.3 `scripts/analyze_perf.py scroll` サブコマンド追加

```
$ python scripts/analyze_perf.py path.jsonl scroll
=== Scroll settle latency ===
   t_rel  visible_first  count  pending  first_ready  all_ready  queue_state
  15.85s  480            16     15        3340 ms     -          pdf_pool: hn=3 n=48 if=3
  32.50s  600            16     4         250 ms      1820 ms    pdf_pool: hn=2 n=15 if=3
```

各 `scroll_settle` に対応する `first_ready` / `all_ready` を join、queue_state も併記。

## Phase 2: 修正 — pdf_pool の promote_to_high_normal

### 2.1 アイデア (Codex R1 P2 で範囲確定)

`reload_queue` / `heavy_io_queue` は既に毎フレーム re-tag されているので**何もしない**。
**修正対象は pdf_pool だけ**。

毎フレーム、`update_keep_range_and_requests` の最後で:
1. 現フレームの `visible_indices[vis_keep_start..vis_keep_end]` の生 idx 集合を作る
   (= 厳密な visible_set、Codex R1 P1-1 対応で sparse / filter 対応)
2. それぞれの idx に対応する pool perf_key (= `pdf_page_perf_key` の決定的な生成式) を
   set 化
3. `pdf_loader::promote_to_high_normal(&visible_keys)` を呼ぶ
4. pool 内部で `normal` VecDeque を走査、`perf_key` が set に入る Job を `high_normal` の
   末尾に移動

dedupe: 前フレームと同じ `visible_keys` セットなら早期 return (cheap hash 比較)。

### 2.2 `PdfWorkerPool::promote_to_high_normal` API

**Codex R2 P2 対応**: 統計の正しい数え方は **lock 下で found_keys を unique 集合として
収集**して、後で計算する。`already_high` は移動前の high_normal 走査で固定する。
priority フィールドも HighNormal に書き換える (P3 polish 反映、dispatch ログで分かる)。

`POOL.get()` 経由 (= 未初期化なら no-op) で per-frame 呼び出しでも pool 起動しない。

```rust
pub struct PromoteStats {
    pub promoted: usize,
    pub already_high: usize,  // pool 内に居て既に HighNormal なジョブ数
    pub not_found_keys: usize,  // keys のうち pool 内に居ないキーの数
}

pub fn promote_to_high_normal(keys: &HashSet<String>) -> PromoteStats {
    let Some(pool) = POOL.get() else {
        return PromoteStats { promoted: 0, already_high: 0, not_found_keys: keys.len() };
    };
    if keys.is_empty() {
        return PromoteStats::default();
    }

    let (promoted_count, already_high, found_keys) = {
        let (mtx, cv) = &*pool.queue;
        let mut q = mtx.lock().unwrap();

        // (1) 既に high_normal に居る match を数える (= 移動前のスナップショット)
        let mut found_keys: HashSet<String> = HashSet::new();
        let mut already_high = 0usize;
        for j in q.high_normal.iter() {
            if let Some(k) = j.perf_key.as_ref()
                && keys.contains(k)
            {
                already_high += 1;
                found_keys.insert(k.clone());
            }
        }

        // (2) normal の単一 pass scan、match した Job を抜き出す
        let mut promoted = Vec::new();
        let mut kept = VecDeque::with_capacity(q.normal.len());
        while let Some(mut j) = q.normal.pop_front() {
            if j.perf_key.as_ref().is_some_and(|k| keys.contains(k)) {
                if let Some(k) = j.perf_key.as_ref() {
                    found_keys.insert(k.clone());
                }
                // priority フィールドも書き換え (dispatch ログで promote 反映)
                j.priority = JobPriority::HighNormal;
                promoted.push(j);
            } else {
                kept.push_back(j);
            }
        }
        q.normal = kept;

        // (3) high_normal の末尾に追加
        let promoted_count = promoted.len();
        for j in promoted.drain(..) {
            q.high_normal.push_back(j);
        }

        cv.notify_all();
        (promoted_count, already_high, found_keys)
    };

    PromoteStats {
        promoted: promoted_count,
        already_high,
        not_found_keys: keys.len() - found_keys.len(),
    }
}
```

**Codex R1 P2-4 / R2 lock 規約**:
- pool mutex を取っている間に perf event や callback を呼ばない
- 移動操作だけ lock 下で完結、perf event emit は lock 外

### 2.3 呼び出し場所

`App::update_keep_range_and_requests` の最後 (= reload_queue 更新の直後) に追加:

```rust
// visible_set の perf_key を集めて pdf_pool に promote 依頼
if self.visible_keys_changed_since_last_promote() {
    let visible_keys = self.collect_visible_pdf_perf_keys();
    let stats = crate::pdf_loader::promote_to_high_normal(&visible_keys);
    if crate::perf::is_enabled() && stats.promoted > 0 {
        crate::perf::event(
            "pdf",
            "pool_promote_visible",
            None,
            self.input_seq,
            &[
                ("promoted", stats.promoted.into()),
                ("already_high", stats.already_high.into()),
                ("not_found", stats.not_found_keys.into()),  // Codex R3 P3 polish
            ],
        );
    }
}
```

`visible_keys_changed_since_last_promote`: 前回の visible_keys のハッシュを保持し、
同一なら true 返さず skip。

### 2.4 collect_visible_pdf_perf_keys の実装 (Codex R1 P1-1 / R2 P2 対応)

**Codex R2 P2 対応**: `vis_keep_window` ではなく **visible 範囲 (= 厳密に画面に
見えてる subset)** を使う。prefetch を HighNormal に上げない (= prefetch は Normal の
ままでよい)。実装は visible_start..visible_end (visible_indices 上の subset)。

```rust
fn collect_visible_pdf_perf_keys(&self) -> HashSet<String> {
    let mut keys = HashSet::new();
    // 厳密に画面に見えてるスライスだけ (= prefetch は含まない)
    if let Some((vis_start, vis_end)) = self.vis_visible_window_strict() {
        for raw_idx in &self.visible_indices[vis_start..vis_end] {
            if let Some(item) = self.items.get(*raw_idx) {
                match item {
                    GridItem::PdfFile(p) => {
                        keys.insert(crate::grid_item::pdf_page_perf_key(p, 0));
                    }
                    GridItem::PdfPage { pdf_path, page_num, .. } => {
                        keys.insert(crate::grid_item::pdf_page_perf_key(pdf_path, *page_num));
                    }
                    _ => {}
                }
            }
        }
    }
    keys
}
```

`visible_indices` の subset を回すので filter / sparse 状態でも正しく動く。

**Codex R3 P2 対応 — visible window helper**:
- `vis_visible_window()` も `vis_keep_window()` も App メソッドとして**存在しない**
- App には [src/app.rs:9657](../src/app.rs) 付近に `vis_visible_start/end` という
  **ローカル変数** がある (extra row margin 含み)。これは promote 用途には少し広めで NG
- → **`App::vis_visible_window_strict()`** という新しい helper を導入する。実装:
  - `scroll_offset_y` + `viewport_height` + `cell_size` + `cols` から行範囲を計算
  - 表示中の `visible_indices[start..end]` 範囲を返す (= 余白行を含まない厳密 visible)
  - `update_keep_range_and_requests` 内で同じ計算をしている箇所があれば、そこから
    extract / 共通化する

**P3 polish (Codex R2)**: `apply_folder_thumb_pin` で PDF 代表ページを pin している場合、
pin 先の page_num は 0 とは限らない (FolderThumbPin の target が PdfPage の場合)。
そのケースは `PdfFile` (page=0 ハードコード) だと正しい perf_key にならない。
ただし pin 経路は marginal なので、初版では PdfFile=page 0 で割り切る。実機で pin 経路
の promote 漏れが顕在化したら別 PR で対応。

### 2.5 race coverage (Codex R1 P2-3 反映)

- **既に dispatched (in-flight) なジョブ**: 救えない (PDFium 中断不可)
  → stats の `not_found` に含まれる
- **next pop 直前のジョブ**: lock 取って移動、worker が pop 時には high_normal にある
- **新規 enqueue 直後**: reload_queue は毎フレーム re-tag されているので priority=true で
  渡され、HighNormal で pool に入る

最終的に「PDF pool の Normal lane に居る visible 対象」だけが promote 対象。これが本 PR の
価値。

## Phase 3: テスト

### 3.1 ユニットテスト

`src/pdf_loader.rs` に追加:
- `promote_to_high_normal_moves_matching_jobs`: keys に match する Normal job を
  high_normal へ移動
- `promote_to_high_normal_leaves_critical_untouched`: Critical は touch しない
- `promote_to_high_normal_handles_empty_keys`: no-op (panic しない)
- `promote_to_high_normal_idempotent`: 同じ keys で 2 回呼んでも safe
- `promote_to_high_normal_perf_key_none_safe`: perf_key=None ジョブは skip

### 3.2 統合検証 (手動)

bookscan で同じシナリオ:
- BS で復帰 → Down キー連打 → 停止
- `--perf-log` 取得
- `python scripts/analyze_perf.py path.jsonl scroll` で first_ready latency 確認
- 期待: 3340 ms → ~500 ms 程度に短縮

## 影響範囲

| ファイル | 変更内容 |
| --- | --- |
| `src/pdf_loader.rs` | `pool_queue_snapshot` + `promote_to_high_normal` API 追加 + `JobQueue.in_flight_started_at` |
| `src/thumb_loader.rs` | `thumb/enqueue` perf event の 3 フィールド分割 |
| `src/app.rs` | scroll_settle 検出 (after-scroll/key) + first_ready/all_ready emit + collect_visible_pdf_perf_keys + promote 呼出 + pool_queue_snapshot 定期 emit + grid_key/grid_wheel 拡張 |
| `scripts/analyze_perf.py` | `scroll` サブコマンド追加 |
| `docs/async-architecture.md` + `CLAUDE.md` | 設計反映 (pool promote_to_high_normal + 新計装) |

## Non-goals

- スクロール中の毎フレーム promote (= dedup cheap だが毎フレ pool lock は不要、頻度は
  visible_keys 変化時のみで十分)
- worker pool 数の変更 (別問題)
- pdf_pool 以外 (susie / image native) の re-tag (現状 PDFium ボトルネックなので後回し)
- `enqueue` の visibility/source 分類を完璧に網羅 (= 主要 3 種でよい、後で追加可)
- in-flight ジョブの中断 (PDFium 制約)

## 実装順序

1. **Phase 2.2-2.4**: `PdfWorkerPool::promote_to_high_normal` + `collect_visible_pdf_perf_keys`
   実装。先に修正だけ入れて手動確認 (perf 計装無しでも体感確認可能)
2. **Phase 1.2**: `thumb/enqueue` のフィールド分割
3. **Phase 1.1**: scroll_settle + first_ready/all_ready + pool_queue_snapshot
4. **Phase 1.3**: analyze_perf.py scroll サブコマンド
5. **Phase 3.1**: ユニットテスト
6. **Phase 3.2**: 手動 perf 検証で before/after 比較
7. **Phase 6**: ドキュメント更新

各 phase で `cargo build --release` + 既存テスト pass を確認。
