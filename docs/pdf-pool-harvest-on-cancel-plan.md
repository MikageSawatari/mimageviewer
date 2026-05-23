# PDF render pool への HarvestOnCancel 導入計画

## 背景・問題

直前の `Add context epoch to PDF render pool` (bf99edec, Codex 5 round 通過) で
context epoch + 3 段階優先度を導入したが、その後の perf log 解析 (2026-05-23 23:13
セッション) で **新たなボトルネック** が判明:

- ユーザが bookscan (12+ PDF 含むフォルダ) で startup → Ctrl+↓ で個別 PDF へ
  → 戻ったとき、一部 cover thumbnail の再 render に **21+ 秒** かかる
- 具体例: Escape artist 196p.pdf の 2 回目 render が `pool_dispatch` で
  `wait_ms=21,695 ms` (rtt=21,740 ms)

### 原因の分解

1. **1 回目 (t=1-3.4s)**: pool_send → pool_dispatch → IPC が走り、`decode_end` が
   t=3.462s に ms=2443 で完了
2. **同時** (t=3.42s): user が AKARI OUTCAST に Ctrl+↓ → `start_loading_items` →
   `bump_full_context_for_load()` → `cancel_token` 立つ + `bump_render_context_epoch()`
3. thumb worker は `pool.execute` の `recv_timeout(50ms)` ループで cancel を検出 →
   **`Err(Interrupted)` を early bail return**
4. **その後** (t=3.46s): pool dispatcher が IPC 結果を受け取って `job.reply.send()` →
   reply receiver は既に drop 済み → silently fail → **render 結果は捨てられる**
5. cache 保存も走らない (load_one_cached が pdf_interrupted 経路で silent return)
6. **2 回目** (t=7.2s, user が bookscan に戻る): cache miss → 再 enqueue
7. pool は他の in-flight IPC (前 navigation 群) を消化中 → 21 秒待ち

つまり **「PDFium が既に処理してくれた高価な render 結果」が、cancel と
タイミングが噛み合うと捨てられる**。これが再 enter 時の再 render 地獄を生む。

### Codex round 1 のレビュー指摘

「次の改善は in-flight IPC 結果を救う方向」に同意した上で、3 案の比較:

| 案 | 概要 | 評価 |
| --- | --- | --- |
| A: pool.execute 全体で cancel bail をやめる | 全 caller (Critical/Background/enumerate も) が cancel 後も待つ | NG。fullscreen / background も巻き込んで「不要処理を待つ」場面が増える |
| B: dispatcher 側で speculative cache save | dispatcher に cache_map/cache key 事情を持ち込む | NG。pdf_loader が cache 層を知るとレイヤー違反 |
| **C (採用)**: 「未開始 stale は今まで通り prune、in-flight は harvest + cache 保存」 | thumb 経路だけ cancel 後も IPC 完了を待つ | **採用**。最小実装、layering 健全 |

## 設計

### 中核アイデア: `CancelWaitPolicy` enum

`PdfWorkerPool::execute` に cancel 待ちポリシーを 1 個追加:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelWaitPolicy {
    /// Default: `cancel.load() == true` が見えた瞬間に `Err(Interrupted)` で early bail。
    /// in-flight IPC があっても結果を待たない (= 結果は dispatcher が reply.send で
    /// silently 捨てる)。enumerate / Critical / background catch-up 用。
    AbortOnCancel,
    /// cancel が立っても **reply を待つ**。in-flight IPC があれば結果を harvest し、
    /// caller (= `load_one_cached`) が cache 保存に進める。
    /// **使うのは thumbnail PDF render の cache-savable 経路のみ**。
    HarvestOnCancel,
}
```

`AbortOnCancel` が既存挙動と完全一致 (= 後方互換)。`HarvestOnCancel` は新規 opt-in。

### キュー内 stale ジョブの扱いは不変

epoch prune (`pool_prune_stale_epoch`) / dispatcher pop 時 cancel skip
(`pool_cancel_queued`) / pop 時 epoch check (`pool_stale_epoch_skip`) は **全て今まで通り**
即 `Interrupted` を返す。**dispatched 済み (= worker が IPC 中) のみ harvest 対象**。

これは pool dispatcher の責任範囲 (queue 管理) と worker thread (実行中) の責任範囲を
明確に分離する。

### `HarvestOnCancel` を使う条件

`load_one_cached` の以下の条件を **全て** 満たすときだけ:

1. **`pdf_page.is_some()`** (= PDF render 経路)
2. **`!skip_cache`** (= cache 保存可能なリクエスト。アイドル品質アップグレードの
   skip_cache=true は対象外)
3. **`cache_map.is_some() && catalog.is_some()`** (= worker が cache save できる前提。
   **Codex round 1 P2 指摘**: `cache_map` だけだと `catalog_arc=None` ケース
   ([app.rs:7091](../src/app.rs)) で適格性誤判定する。実際の cache save 条件は
   `catalog.is_some() && cache_decision.should_cache(...)` ([thumb_loader.rs:1968](../src/thumb_loader.rs))
   なので両方必要)
4. **`req.priority=true` の場合 (HighNormal) も対象**: Codex round 1 確認回答で、可視
   セルでも cache 保存価値あり。Priority は queue 並び順だけに影響、IPC 開始済みなら
   abort しても worker slot は解放されないので harvest 一択

なお `cache_decision.should_cache()` の動的判定 (decode 時間や size 閾値) は IPC 完了後に
しか判定できないので、policy 選択時点では使わない (= cache 保存しない判定でも harvest
自体は走るが、無害)。

`HarvestOnCancel` を使わない経路 (= `AbortOnCancel` 継続):
- `enumerate_pages_with_cancel` (Critical / background 両方): IPC 自体が軽量 (列挙のみ)、
  かつ列挙結果は cache 保存ロジックがない
- `enumerate_pages_async` (UI nav Critical): 即時応答が UX 価値、harvest 不要
- `get_document_info`: indexer が呼ぶ background、cache 保存ロジック無し
- `render_page` from fullscreen (`app.rs:13517`): Critical は予約 worker で即実行、
  Normal prefetch は `fs_pending.cancel` で functional には十分
- `render_page` from bulk cache creator (`app.rs:17425, 17486`, `thumb_loader.rs:2242`):
  既に直接 cache 保存しており、cancel された場合は中断したい意図
- `render_page` from neighbor prefetch (`thumb_loader.rs:1290`): background

つまり **`HarvestOnCancel` は `load_one_cached` 内の 1 箇所のみ**で使う。

### 実装ポイント: cache save の cancel-awareness

`HarvestOnCancel` で IPC 結果を受け取った後、`load_one_cached` は既存ロジックで
cache save まで進む。cancel が立っていても WebP encode + DB insert は走らせる
(= harvest の意義)。

ただし `tx.send(ThumbMsg)` の 1 個目 (display ColorImage) は cancel 中は無意味:
- UI は既に items_gen を bump 済み or 次 frame で bump 予定
- 旧 items_gen で送られた ThumbMsg は `poll_thumbnails` で mismatch filter される

なので tx.send は無害だが、ログに「harvested + cached after cancel」を残すと
perf 分析しやすい。

### perf イベント追加

| イベント | 発火位置 | 用途 |
| --- | --- | --- |
| `pool_cancel_harvest_wait` | `pool.execute` 内で cancel=true 検出時に AbortOnCancel ではなく待ち継続を選んだ瞬間 | harvest 機構が発動した回数を計測 |
| `pdf_thumb_cache_saved_after_cancel` | `load_one_cached` の cache save 完了直後、cancel が立っていた場合 | 実際に投資回収できた件数を計測 |

これで「21 秒待ち地獄」が解消されたか perf log で定量化できる。

## 実装計画

### Phase 1: `src/pdf_loader.rs`

#### 1.1 `CancelWaitPolicy` enum 追加 + `execute` シグネチャ拡張

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelWaitPolicy {
    AbortOnCancel,
    HarvestOnCancel,
}

fn execute(
    &self,
    request: &[u8],
    cancel: Option<&Arc<AtomicBool>>,
    priority: JobPriority,
    perf_key: Option<String>,
    context_epoch: u64,
    cancel_policy: CancelWaitPolicy,  // NEW
) -> std::io::Result<Vec<u8>>
```

#### 1.2 `execute` の recv ループ修正

```rust
let mut harvest_logged = false;
loop {
    match reply_rx.recv_timeout(Duration::from_millis(50)) {
        Ok(result) => return result,
        Err(Timeout) => {
            let cancelled = cancel.is_some_and(|c| c.load(Ordering::Relaxed));
            if !cancelled { continue; }
            match cancel_policy {
                CancelWaitPolicy::AbortOnCancel => {
                    // 既存ロジック: perf event + return Interrupted
                    return Err(...);
                }
                CancelWaitPolicy::HarvestOnCancel => {
                    // 一度だけ perf イベント (harvest 待ち発動)
                    if !harvest_logged {
                        harvest_logged = true;
                        if perf::is_enabled() {
                            crate::perf::event(
                                "pdf",
                                "pool_cancel_harvest_wait",
                                perf_key.as_deref(),
                                0,
                                &[("waited_ms", ...)],
                            );
                        }
                    }
                    // 待ち継続 (= 次の iteration)
                }
            }
        }
        Err(Disconnected) => return Err(...),
    }
}
```

#### 1.3 `render_page` シグネチャ拡張

```rust
pub fn render_page(
    pdf_path: &Path,
    page_num: u32,
    target_px: u32,
    password: Option<&str>,
    cancel: Option<Arc<AtomicBool>>,
    priority: JobPriority,
    context_epoch: u64,
    cancel_policy: CancelWaitPolicy,  // NEW
) -> std::io::Result<RenderResult>
```

呼び元更新 (= `render_page` の全 caller、Codex round 2 で網羅確認済み):

| ファイル:行 | 呼び元 | 設定する policy |
| --- | --- | --- |
| `thumb_loader.rs:~1693` | `load_one_cached` (UI thumb worker、`process_load_request` 経由) | `process_load_request` で計算した値 (= cache 可能なら `HarvestOnCancel`) |
| `thumb_loader.rs:1290` | `process_neighbor_prefetch` | `AbortOnCancel` (background) |
| `thumb_loader.rs:2242` | `build_and_save_one_pdf` | `AbortOnCancel` (bulk cache creator) |
| `app.rs:13517` | fullscreen load (current=Critical / prefetch=Normal) | `AbortOnCancel` (current は Critical 予約で即実行、prefetch は fs_pending で制御) |
| `app.rs:17425, 17486` | bulk cache creator | `AbortOnCancel` |

**`pool.execute` を直接呼ぶ非 render 経路** (= `render_page` を経由しない、Codex round 2
P3 polish 指摘):

| ファイル:行 | 呼び元 | 設定する policy |
| --- | --- | --- |
| `pdf_loader.rs:~1736` | `get_document_info` (indexer 経由) | `AbortOnCancel` (background) |
| `pdf_loader.rs:~1774` | `enumerate_pages_with_cancel` (background catch-up) | `AbortOnCancel` |
| `pdf_loader.rs:~1996` | `enumerate_pages_async` (UI nav Critical) | `AbortOnCancel` (Critical は即時応答 UX 重視) |

**`render_page_async`** ([app.rs:13856](../src/app.rs) / [pdf_loader.rs:1921](../src/pdf_loader.rs)):
fullscreen PDF zoom rerender 専用 (cache 保存ロジック無し)。本 PR の対象外。
シグネチャも変更しない (= 内部で AbortOnCancel 相当の挙動を維持)。

### Phase 2: `src/thumb_loader.rs`

#### 2.1 `process_load_request` で policy 計算 → `load_one_cached` に渡す

**Codex round 2 P2**: `skip_cache` は `load_one_cached` のシグネチャに無く
`req.skip_cache` でしかアクセスできない ([thumb_loader.rs:1636](../src/thumb_loader.rs))。
そこで policy 計算は呼び元 `process_load_request` で行い、`load_one_cached` に
1 個の `CancelWaitPolicy` enum で渡す方が clean。

`process_load_request` 内 (`load_one_cached` 呼び出し直前):
```rust
// PDF render 経路の cache-savable な request だけ harvest 対象。
// **これは静的な harvest gate** であって literal cache save gate ではない
// (実際の save 条件は `cache_decision.should_cache()` の動的判定を含む)。
// 静的に明らかに cache 保存不可能なケース (skip_cache / catalog 無し / cache_map 無し)
// だけ AbortOnCancel に倒す。
let cancel_policy = if req.pdf_page.is_some()
    && !req.skip_cache
    && catalog_ref.is_some()
{
    crate::pdf_loader::CancelWaitPolicy::HarvestOnCancel
} else {
    crate::pdf_loader::CancelWaitPolicy::AbortOnCancel
};
load_one_cached(
    // ... existing args ...
    cancel_policy,  // NEW
);
```

`load_one_cached` のシグネチャに `cancel_policy: CancelWaitPolicy` を追加し、内部の
`render_page` 呼び出しに渡す。

`cache_map` 引数は `Some(cache_map)` で渡されるパターンが標準で、`process_load_request`
からの経路では常に Some。`load_one_cached` を他から直接呼ぶ経路 (もしあれば) は
AbortOnCancel を渡すよう更新する (実装時に grep で網羅確認)。

#### 2.2 cache save 後の perf イベント

**Codex round 1 P3-1 指摘**: cancel チェックは `cache_map.insert` の **後**に取る。
encode / DB save 中 (数十 ms) に cancel が flip する可能性もあるため、insert 完了
直後にチェックする方が「投資回収に成功した」セマンティクスを正確に反映する。

```rust
// ... WebP encode → catalog.save → cache_map.write().insert(...) ...
if perf::is_enabled() && cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
    crate::perf::event(
        "pdf",
        "pdf_thumb_cache_saved_after_cancel",
        Some(filename),
        input_seq,
        &[("idx", serde_json::Value::from(idx))],
    );
}
```

具体的な挿入位置は実装時に既存 cache save の場所 (`thumb_loader.rs:1977` 付近) を
読んで決める。

### Phase 3: Codex P2 follow-up の整合性確認

直前のコミット (8d496095) で `pdf_interrupted` 経路は `canceled=true` を送るように
した。Harvest 成功時 (= IPC 結果取得 + cache save) は `pdf_interrupted` path に入らず
正常完了経路を通る。**cancel が立っていても items_gen mismatch + bounds guard で
UI 側が無視する**ため安全。

具体的には `poll_thumbnails` ([app.rs:9259](../src/app.rs)) の入口で items_gen
mismatch をチェックし、その直後の bounds guard ([app.rs:9266](../src/app.rs)) でも
古い idx を弾く。これらは副作用前 (= `requested.remove` や `thumbnails[]` 書き換え前)
に走る。

ただし 1 点確認: ZIP/PDF 仮想フォルダ open 経路 ([app.rs:5789, 5995](../src/app.rs))
では cancel と items 状態クリアが先で items_generation の bump が後。この短い window
では items_gen mismatch ではなく bounds guard 側が古い ThumbMsg を捨てる
(Codex round 1 P3-2 指摘)。どちらも UI に副作用は出ない。

### Phase 4: テスト

#### 4.1 ユニットテスト

`src/pdf_loader.rs` の既存 `#[cfg(test)] mod tests` に追加:

- `cancel_wait_policy_abort_returns_interrupted_immediately`: cancel + AbortOnCancel で
  early bail を確認
- `cancel_wait_policy_harvest_waits_for_reply`: HarvestOnCancel で cancel が立っても
  reply が来るまで待つ (mock reply で確認)

#### 4.2 統合検証

`--perf-log` で bookscan の Ctrl+↑↓ 反復シナリオを再現:

- `pool_cancel_harvest_wait` が発火する
- `pdf_thumb_cache_saved_after_cancel` が発火する
- 再エントリ時の cover ready 時間が大幅短縮 (21 秒 → 数百 ms)

`scripts/analyze_perf.py` の出力 (counts of new events) で定量確認。

### Phase 5: ドキュメント更新

#### 5.1 `docs/async-architecture.md`

§2.3 の `pdf_pool.queue` 説明に CancelWaitPolicy / Harvest の 1 行追加。
§3 のキャンセル規約に新セクション or 既存 PdfWorkerPool epoch セクションに追記。

#### 5.2 `CLAUDE.md` の PDF レンダ pool セクション

epoch 説明の後に Harvest mode を追記。

#### 5.3 本ドキュメント

実装完了後、Codex review 状況と実装上の補足を末尾に追加 (前回 Plan ドキュメントと
同じパターン)。

## 影響範囲とリスク

| 領域 | 変更内容 |
| --- | --- |
| `src/pdf_loader.rs` | CancelWaitPolicy enum + execute / render_page シグネチャ拡張 + perf event 追加 |
| `src/thumb_loader.rs` | load_one_cached での policy 選択 + cache save 後 perf event |
| `src/app.rs` | 各 render_page 呼び出しに `AbortOnCancel` 引数追加 (= 既存挙動維持) |
| `docs/async-architecture.md` + `CLAUDE.md` | 設計反映 |

### リスク

| リスク | 対策 |
| --- | --- |
| HarvestOnCancel で thumb worker が IPC 完了まで blocked → 新フォルダの作業が遅れる | start_loading_items は新 worker を別途 spawn する (既存挙動)。old worker の harvest と並列処理可能。最大 2 秒程度の overlap で thread 数が一時的に倍。worker thread は軽量なので実害無し |
| 新世代の duplicate render 防止が in-memory 上では効かない (Codex round 1 P3-3) | `start_loading_items` で新しい `cache_map` snapshot が作られる ([app.rs:7091](../src/app.rs))。old worker が old cache_map に harvest 結果を書いても、new worker は new cache_map を見るので in-memory hit にならない。**ただし DB レベルでは最終的に save される**ので、ユーザがさらに次回戻ったときには cache hit になる。1 世代だけ duplicate render が走るが、新世代 worker は通常 cache_decision の通常判定で動くだけで実害は限定的 |
| HarvestOnCancel 中に user が再度 cancel (連打) しても抜けない | harvest 待ちは IPC 1 件 (最大 ~2 秒) で必ず終わるので問題なし。連打しても新 cancel が立つだけで harvest 完了は変わらない |
| cache save が cancel 中の write でデッドロック | cache_map は RwLock、cache save は短時間 write lock。cancel 後 write も既存挙動 (cancel 経由で抜けないコード経路) と同じく安全 |
| dispatcher pop 後 epoch stale で即 Interrupted を返すケース | reply は Interrupted エラーなので caller は IPC 経路に入らず、harvest 機構も発動しない。既存挙動と等価 |
| 古い display ColorImage の 1st tx.send が UI に届く | items_gen mismatch で `poll_thumbnails` が drop。副作用無し |

### Non-goals

- `enumerate_pages_async` の harvest 対応 (enumerate は cheap、cache 保存ロジック無し)
- bulk cache creator の harvest 対応 (cancel は明示的中断意図)
- pool dispatcher の挙動変更 (引き続き queue 管理に専念)
- in-flight IPC のキャンセル (PDFium 制約は本 PR でも残る)

## 期待効果

bookscan rapid Ctrl+↑↓ シナリオでの perf log 期待値:

| 指標 | 修正前 (現状) | 期待 |
| --- | --- | --- |
| bookscan 再エントリ時の cover 最遅 ready | 21 秒 | 数百 ms (cache hit に変わる) |
| `pool_recv` で rtt > 5 秒の件数 | 30 件超 | 数件以下 (= 真に 5 秒超 IPC のみ) |
| 同じ PDF の 2 回目 render | 多発 | ほぼゼロ (1 回目で cache 保存される) |
| `pool_cancel_harvest_wait` (新規) | N/A | 数十件 (= harvest が機能) |
| `pdf_thumb_cache_saved_after_cancel` (新規) | N/A | 数十件 (= 投資回収) |
| `pool_cancel_requester` | ~26 | 大幅減 (harvest が代わる) |

## 実装順序

1. **Phase 1.1-1.3**: `CancelWaitPolicy` 追加 + `execute` / `render_page` シグネチャ
   拡張。既存呼び元は全て `AbortOnCancel` で呼ぶよう更新 (= 挙動変化なし)
2. **Phase 2.1**: `load_one_cached` で `HarvestOnCancel` に切替
3. **Phase 2.2 / 1.2 perf**: 新 perf イベント 2 種追加
4. **Phase 4.1**: ユニットテスト
5. **Phase 4.2**: 手動検証 (bookscan perf log で改善確認)
6. **Phase 5**: ドキュメント更新

各 phase で `cargo build --release` + 既存テスト pass を確認、最後に手動 perf 検証。
