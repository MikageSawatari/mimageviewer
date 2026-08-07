# 高速スクロール中の先読み抑制 (Codex R3 反映版)

## 背景

直近の perf log (2026-05-24、scroll_settle 計装 + promote_to_high_normal 導入後)
では、Down キー連打で 444 idx を 2 秒で移動、停止 (settle) 後の 1 枚目サムネ
表示 latency が以下のように観測された:

| settle | preload | first_ready | all_ready | 解釈 |
|---|---|---|---|---|
| seq 4 | 0 | **1417 ms** | - | 改善 (旧 21 秒台) |
| seq 6 | 0 | **268 ms** | 310 ms | cache hit、爆速 |
| **seq 8** | 0 | **9504 ms** | 27553 ms | 悪化 |

settle 時の pool snapshot は `hn=6 n=2 if=2`。in-flight 2 は **スクロール中に
enqueue された prefetch (priority=False) が dispatcher に拾われて IPC 中** で、
これは PDFium cancel 不可なので最低 1.5 秒待ち × 残務処理が visible 表示を
ブロックする。

つまり **「prefetch jobs がスクロール中に pool まで流れる → in-flight 占有」**
が残るボトルネック。

## 設計方針

> 可視範囲外の先読みを、可視範囲のサムネイルがすべて揃っていて、かつ 100 ms の
> アイドル時間経過後に開始する

(ユーザ提案、2026-05-24)

スクロール中 / visible 待ち中は **prefetch enqueue 自体を抑制**することで、
pool に prefetch が流れる前に止める。in-flight として握られる前に止めるので
cancel 不可問題に影響されない。

### 抑制条件 (両方を満たすときだけ prefetch を「許可」する)

1. **scroll idle**: 最後のスクロール (offset 変化) から **≥ 100 ms** 経過
2. **visible all ready**: 厳密 visible 範囲の `thumbnails[i]` が全て `Loaded`
   または `Failed` (= 終端状態。Pending / Evicted / Requested を含まない)

両方満たすなら従来通り prefetch enqueue。
どちらか満たさないなら **prefetch (= `req.priority == false`) を enqueue しない**。
visible (= `req.priority == true`) は常に enqueue する。

### 期待効果

- **スクロール中**: visible だけが reload_queue / pool に流れる。pool は visible
  集中処理、in-flight backlog 蓄積無し
- **stop 直後 (= 100 ms 内)**: prefetch 未投入のまま visible の処理を進める
- **stop 後 100 ms 経過 + visible 揃った**: prefetch 再開
- **cache hit ケース**: visible が数 ms で Loaded → 100 ms 待ちはあるが、prefetch は
  どうせ次フレで開始するので体感差なし
- **重 PDF visible**: visible が 1 件 Loading 残ったまま長時間 → prefetch 永久
  停止になり得る。**`Failed` も終端扱い**にして部分救済する (= 完全停止しない)

### スコープ (Codex R1 P2 反映)

本 PR の抑制対象は **grid thumbnail prefetch のみ** (= `update_keep_range_and_requests`
内の `reload_queue` / `heavy_io_queue` 経路)。以下の Normal PDF render 経路は
**範囲外** (それぞれ独自タイミングで動くので grid scroll とは別問題):

- **Neighbor PDF prefetch** ([app.rs:6115, 6154](../src/app.rs) → [thumb_loader.rs:1309](../src/thumb_loader.rs))
  : load_pdf_as_folder の前後 PDF 温め。Enter 直後の UX で別 grace あり。
- **Idle upgrade** ([app.rs:9922, 10076, 10231](../src/app.rs)): アイドル時の
  画質アップグレード。そもそも idle 中しか走らないので scroll とは衝突しない。

これらまで gate するべきかは別問題。本 PR では現状維持 → 観測続行で判定。

### 既存機構との関係

`pdf_prefetch_grace_until` ([app.rs:2818](../src/app.rs)): PDF as folder の
Enter 直後 grace。「現在ページを最優先、prefetch を一瞬抑える」用途。今回の
ガードと別目的なので**両方残す** (= 論理 AND で動く、片方でも block すれば抑制)。

## Phase 1: 実装

### 1.1 `App::prefetch_allowed_now()` 追加

`decide_prefetch_allowed` free function を呼ぶ薄い wrapper。実 logic は (1.7) 参照。

```rust
fn prefetch_allowed_now(&self, vis_first: usize, items_per_page: usize, vis_count: usize) -> bool {
    // visible 範囲の Pending 数を数える
    let vis_end = vis_first.saturating_add(items_per_page).min(vis_count);
    let mut visible_pending = 0usize;
    for &raw_idx in &self.visible_indices[vis_first..vis_end] {
        match self.thumbnails.get(raw_idx) {
            Some(crate::grid_item::ThumbnailState::Loaded { .. })
            | Some(crate::grid_item::ThumbnailState::Failed) => {}
            _ => visible_pending += 1,
        }
    }
    // 実 logic は free function に切り出し (1.7 参照、ユニットテスト容易)
    matches!(
        decide_prefetch_allowed(
            std::time::Instant::now(),
            self.last_prefetch_scroll_at,
            visible_pending,
        ),
        PrefetchDecision::Allow { .. }
    )
}
```

引数 `vis_first` / `items_per_page` / `vis_count` は `update_keep_range_and_requests`
内で既に計算されているので、その値を流用する (= 二重計算回避)。

### 1.2 enqueue ループに gate 追加 + **既存 queued prefetch の prune**

**Codex R1 P1 対応 (重要)**: 旧案では新規 enqueue のみ block していたが、
`reload_queue` / `heavy_io_queue` に既に居る prefetch も worker がいずれ pick して
pool.normal に流す。これでは抑制の意味がない。→ **q.retain で queued prefetch も
削る**必要がある。

`update_keep_range_and_requests` の reload_queue / heavy_io_queue 構築ループで:

```rust
let prefetch_ok = self.prefetch_allowed_now(vis_first, items_per_page, vis_count);

// **Codex R3 P2-2 対応**: 既存 priority field は古い (= 前フレの visible_raw 基準)。
// retain 内で「now_visible」を inline 計算して判定する。priority field の re-tag
// は別ループで行う必要なし (= retain 内の判定が事実上の re-tag を兼ねる... と
// したいが、retain は immutable borrow のため、再代入は別の iter_mut loop で行う)。
//
// 順序: (1) retain で keep_set / prefetch gate を一括判定 → (2) iter_mut で priority 再代入。
// retain 内では (now_visible || skip_cache) で keep ok とする。
q.retain(|r| {
    let keep_set_ok = keep_set.contains(&r.idx);
    // **inline visibility check** (= priority field を信用しない、Codex R3 P2-2)
    let now_visible = r.idx >= visible_raw_start && r.idx < visible_raw_end;
    // skip_cache=true は idle upgrade で本 PR scope 外 (Codex R2 P2-1)
    let is_grid_prefetch = !now_visible && !r.skip_cache;
    let blocked = !prefetch_ok && is_grid_prefetch;
    let should_keep = keep_set_ok && !blocked;
    if !should_keep {
        requested.remove(&r.idx);
    }
    should_keep
});

// 既存の re-tag ループ (priority field を最新 visible_raw に同期)
for r in q.iter_mut() {
    r.priority = r.idx >= visible_raw_start && r.idx < visible_raw_end;
}

// 新規 enqueue gate
for i in vis_keep_start..vis_keep_end {
    // ... 既存の早期 continue (keep_set / requested / Loaded skip など) ...

    req.priority = i >= visible_raw_start && i < visible_raw_end;

    // **NEW**: prefetch (= visible 外) が抑制中なら新規 enqueue しない
    if !req.priority && !prefetch_ok {
        continue;
    }
    // 既存 pdf_prefetch_blocked との交差も維持
    if pdf_prefetch_blocked && !req.priority && req.pdf_page.is_some() {
        deferred_pdf_prefetch = true;
        continue;
    }
    // ... 既存 enqueue ロジック ...
}
```

`requested.remove` で prune したアイテムは次フレに `Pending/Evicted` 状態のままなので、
gate 解放後 update_keep_range_and_requests が再走したときに新規 enqueue 経路で再投入される。

### 1.3 早期 scroll 検出 (Codex R1 P2 / R2 P2 対応)

**問題**: `last_scroll_event_at` は `update_scroll_settle_state` (= render_grid 後、
[app.rs:19314](../src/app.rs)) でのみ更新される。同フレーム内では:
- `update_keep_range_and_requests` ([app.rs:18204](../src/app.rs)) で gate 判定
- `process_scroll` ([app.rs:18334](../src/app.rs)) と `handle_keyboard` で
  `scroll_offset_y` mutate
- `render_grid` ([app.rs:18794](../src/app.rs)) で scrollbar dragback 反映
- `update_scroll_settle_state` で last_scroll_event_at 更新

→ **gate 判定時点 (= 18204) では同フレームのスクロール入力は scroll_offset_y に
まだ反映されてない**。offset 比較しても変化ゼロで「idle」と誤判定する。

**修正 (Codex R2 P2 / R3 P2-1 対応)**: 専用 timestamp **`last_prefetch_scroll_at`**
を導入。既存の `last_scroll_event_at` (= settle 用) は `emit_scroll_settle_event`
で clear されるため backstop 計算に使えない。

`last_prefetch_scroll_at` は:
- `App::update` 冒頭の input intent detection (= ctx.input ベース。thumbnail grid の
  raw Touch Move も含む) で prefetch 用 timestamp を set
- `update_scroll_settle_state` の offset 変化検出でも fallback として set
  (= scrollbar drag などキー以外の経路の保険)
- 一覧の touch drag は `scroll_offset_y` を行境界に保ったまま描画端数だけ動くため、
  touch command 適用境界から move / hold / release ごとに prefetch / settle / idle-upgrade の
  各 timestamp を明示的に set
- **clear しない**。backstop 3 秒は前回の scroll から経った時間で判定する
- folder 切替 (= `start_loading_items`) では `None` ではなく **`Some(now)`** にリセット
  (= 「直前にスクロールしたのと同じ扱い」で新コンテキストでも gate を効かせる、Codex R4 確認)

```rust
// App::update の早い段階 (= update_keep_range_and_requests より前)
fn detect_scroll_input_intent(&mut self, ctx: &egui::Context) {
    let scrolling = ctx.input(|i| {
        // wheel / trackpad
        i.raw_scroll_delta.length() > 0.1
        // arrow keys (= grid_key sel 変更)
        || i.key_pressed(egui::Key::ArrowDown)
        || i.key_pressed(egui::Key::ArrowUp)
        || i.key_pressed(egui::Key::PageDown)
        || i.key_pressed(egui::Key::PageUp)
        || i.key_pressed(egui::Key::Home)
        || i.key_pressed(egui::Key::End)
    });
    if scrolling {
        self.last_prefetch_scroll_at = Some(std::time::Instant::now());
    }
}

// update_scroll_settle_state 側の fallback (offset 変化検出時にも書く)
if (cur_offset - self.prev_scroll_offset_y).abs() > 0.5 {
    self.last_scroll_event_at = Some(now);          // settle 用 (既存)
    self.last_prefetch_scroll_at = Some(now);        // prefetch gate 用 (新規、clear されない)
    ...
}
```

input intent detection は **scrollbar ドラッグを直接拾わない** が、この経路では offset 変化が
発生するので fallback で 1 フレ遅れて last_prefetch_scroll_at が更新される。実害は 1 frame の
prefetch 漏れだけで、q.retain で次フレ確実に prune される (= 1.2 で正しい priority タイミング後に
prune)。一覧の touch drag は offset が変わらない端数フレームを持つため、この fallback には頼らない。

### 1.4 backstop (Codex R1 P2-3 対応、`decide_prefetch_allowed` 内に統合)

visible item が永久 Pending → prefetch 永久停止を防ぐため、絶対 timeout を設定。
**実装は (1.7) の `decide_prefetch_allowed` 内**。3 秒経過したら無条件 Allow を返す
(`AllowReason::Backstop3s`)。これにより最悪 3 秒で先読み再開、permanent stall 不可。

`last_prefetch_scroll_at` は **`emit_scroll_settle_event` で clear されない** ので
backstop の計時起点として安定 (= Codex R3 P2-1 対応の本丸)。

### 1.5 gate 解放時の repaint 確保

scroll idle 100 ms / backstop 3 秒 タイマーで自然に発火するように:

```rust
if !prefetch_ok {
    // scroll idle 不足が原因なら、100 ms 経過時点で repaint してこの関数を再走させる
    if let Some(t) = self.last_prefetch_scroll_at {
        let elapsed = t.elapsed();
        let remaining = std::time::Duration::from_millis(100).saturating_sub(elapsed);
        if !remaining.is_zero() {
            ctx.request_repaint_after(remaining);
        }
        // backstop 用にも repaint (= visible 永久 Pending でも 3 秒で必ず動く)
        let backstop_remaining = std::time::Duration::from_secs(3).saturating_sub(elapsed);
        if !backstop_remaining.is_zero() {
            ctx.request_repaint_after(backstop_remaining);
        }
    }
    // visible 未完了が原因の場合は thumb worker の ready 経路が repaint を投げるので追加不要
}
```

### 1.6 perf 計装

**Codex R1 P3 / R2 P3 対応**: 実際に何件 suppress したかを残す。「ガード false」だけ
emit しても無意味 (= visible 全部終わってて prefetch 候補が無いフレームを大量
カウントするのは無駄)。`allow_reason` (= なぜ unblock したか) や `backstop_hit`
(= 3 秒 backstop 発動したか) も含める。

- `ui/prefetch_suppressed`: 抑制 candidate が 1 件以上ある frame のみ emit。
  - 「前フレーム allowed → 今フレーム not allowed」遷移時 (= 始点)
  - 一定間隔 (= 200ms) で 1 回 (= 抑制中の状態 trace)
  - 抑制解放時の 1 回 (= 終点、`allow_reason` 付き)
  ```json
  {
    "cat": "ui",
    "kind": "prefetch_suppressed",
    "scroll_idle": false,
    "visible_ready": true,
    "scroll_idle_ms_remaining": 73,
    "visible_pending": 0,
    "suppressed_regular": 12,
    "suppressed_heavy": 3,
    "pruned_from_queue": 8,
    "transition": "start",   // start / continue / end
    "allow_reason": null,    // unblock 時に "scroll_idle" / "visible_ready" / "backstop_3s"
    "backstop_hit": false
  }
  ```

### 1.5 動作確認用 analyze_perf.py 既存 scroll コマンド拡張

`scripts/analyze_perf.py scroll` に「scroll_settle 直前の `prefetch_suppressed`
イベント有無」を併記。すなわち抑制が効いた settle / 効かなかった settle を
見分け可能にする (P3 polish)。

## Phase 2: テスト

### 2.1 ユニットテスト

`prefetch_allowed_now` 内の判定ロジックを **`decide_prefetch_allowed` free function**
に切り出して単体テスト。`Instant::now()` を引数で取り、テスト可能に。

```rust
/// 戻り値: (allowed, allow_reason or block_reason)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrefetchDecision {
    Allow { reason: AllowReason },
    Block { reason: BlockReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AllowReason {
    NoScrollYet,
    ScrollIdleAndVisibleReady,
    Backstop3s,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockReason {
    ScrollNotIdle { elapsed_ms: u64 },
    VisibleStillLoading { pending: usize },
}

const PREFETCH_IDLE_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(100);
const PREFETCH_BACKSTOP: std::time::Duration = std::time::Duration::from_secs(3);

pub(crate) fn decide_prefetch_allowed(
    now: std::time::Instant,
    last_prefetch_scroll_at: Option<std::time::Instant>,  // R3 P2-1: settle で clear されない専用
    visible_state_pending: usize,  // 0 = 全部 Loaded/Failed
) -> PrefetchDecision {
    // (1) backstop: scroll から 3 秒経ったら無条件 allow (visible Pending 永久 stall 防止)
    if let Some(t) = last_prefetch_scroll_at
        && now.saturating_duration_since(t) >= PREFETCH_BACKSTOP
    {
        return PrefetchDecision::Allow {
            reason: AllowReason::Backstop3s,
        };
    }
    // (2) scroll idle: 最後の scroll から 100ms 未満なら block
    if let Some(t) = last_prefetch_scroll_at {
        let elapsed = now.saturating_duration_since(t);
        if elapsed < PREFETCH_IDLE_THRESHOLD {
            return PrefetchDecision::Block {
                reason: BlockReason::ScrollNotIdle {
                    elapsed_ms: elapsed.as_millis() as u64,
                },
            };
        }
    }
    // (3) visible ready
    if visible_state_pending > 0 {
        return PrefetchDecision::Block {
            reason: BlockReason::VisibleStillLoading {
                pending: visible_state_pending,
            },
        };
    }
    let reason = if last_prefetch_scroll_at.is_none() {
        AllowReason::NoScrollYet
    } else {
        AllowReason::ScrollIdleAndVisibleReady
    };
    PrefetchDecision::Allow { reason }
}
```

テスト (Codex R2 P3 boundary 含む):

- scroll never happened (None) + visible 0 pending → Allow { NoScrollYet }
- scroll 50 ms ago + visible 0 pending → Block { ScrollNotIdle }
- scroll exactly 100 ms ago + visible 0 pending → Allow (boundary)
- scroll 99 ms ago + visible 0 pending → Block
- scroll 200 ms ago + visible 5 pending → Block { VisibleStillLoading }
- scroll 200 ms ago + visible 0 pending → Allow
- scroll 2999 ms ago + visible 5 pending → Block (backstop 未到達)
- scroll exactly 3000 ms ago + visible 5 pending → Allow { Backstop3s } (boundary)
- visible 空 (pending=0) → Allow

### 2.2 統合検証 (手動)

bookscan で同じシナリオ (BS → Down キー連打 → 停止) で:
- `analyze_perf.py scroll` で first_ready latency 確認
- `prefetch_suppressed` event 数を count (= 抑制が機能した frame 数)
- pool_queue_snapshot の `in_flight` が抑制中に小さい値を維持してるか確認

期待: first_ready が 9504 ms → 1500-2000 ms (= PDFium 1 page の単純な時間) に短縮。

## 影響範囲

| ファイル | 変更内容 |
| --- | --- |
| `src/app.rs` | `prefetch_allowed_now` 追加 + `update_keep_range_and_requests` の enqueue gate + request_repaint_after + perf event |
| `scripts/analyze_perf.py` | `scroll` サブコマンド拡張 (= 抑制 event 表示) |
| `docs/async-architecture.md` + `CLAUDE.md` | 設計反映 (汎用 prefetch ガード) |

## リスク

| リスク | 対策 |
| --- | --- |
| visible に 1 件 Failed/Pending が永久残留 → prefetch 完全停止 | **backstop**: scroll から 3 秒経過したら無条件 allow (= 永久 stall 不可、Codex R1 P2-3 対応) |
| scroll が頻繁に微小変化 (= 1px) → ずっと idle にならない | offset 変化検出を `> 0.5` 既存実装で十分。微小変化は無視される |
| cache hit folder で 100ms 待ちが体感ダウン? | visible 数 ms で Loaded → 次フレで gate 通過。1 frame 遅れだけで体感差なし |
| `pdf_prefetch_grace_until` との干渉 | 両 gate は AND で動く (= どちらかが block すれば抑制)、両立 |
| settle 後すぐ次のスクロールに移行 (= ユーザが操作続行) | scroll 検出で gate が再 close、prefetch が再開しない。これは意図通り |
| **queue 内の既存 prefetch が pick されて pool に流れる** (Codex R1 P1) | **q.retain で priority=false を prune**。new enqueue だけでなく既存 queued も drop |
| 1 フレーム遅れた timestamp で初フレに prefetch 漏れ (Codex R1 P2-1) | **`App::update` 冒頭で `detect_scroll_input_intent` (ctx.input ベース)** で `last_prefetch_scroll_at` を即時更新。`update_scroll_settle_state` 側の offset 検出は scrollbar drag 等の fallback として残す |
| settle で `last_scroll_event_at` clear → backstop 計時起点喪失 (Codex R3 P2-1) | **専用 `last_prefetch_scroll_at` 導入**。settle telemetry で clear されない |
| q.retain が古い priority field で判定 → 直前 visible 化したアイテム巻き込み prune (Codex R3 P2-2) | **inline `now_visible = idx in visible_raw` 判定**を retain 内で行う、priority field を信用しない |
| Neighbor PDF prefetch / Idle upgrade で in-flight が再蓄積する可能性 | 本 PR scope 外。観測続行で必要に応じて別 PR |

## Non-goals

- スクロール中の動的 priority 切替 (= promote_to_high_normal で対応済み)
- worker 数の動的変更
- prefetch 量 (= keep_range サイズ) の調整 (別問題)
- visible 完了の "100% 厳密判定" (= keep_set 全体ではなく visible のみで判定。
  prefetch は keep_set ⊃ visible の差集合、ここは抑制対象)

## 実装順序

1. **Phase 1.1**: `decide_prefetch_allowed` free function + `PrefetchDecision` enum
   (Codex R1 P2-3 backstop / R2 P3 boundary 対応)
2. **Phase 1.3**: `App::update` 冒頭で `detect_scroll_input_intent` 呼び出し
   (Codex R2 P2 対応、ctx.input ベース)
3. **Phase 1.2**: q.retain で queued prefetch prune (`!r.priority && !r.skip_cache`、
   Codex R2 P2-1 対応) + 新規 enqueue gate
4. **Phase 1.5**: request_repaint_after で 100ms / 3 秒境界 wake-up 確保
5. **Phase 1.6**: `prefetch_suppressed` perf event (transition / allow_reason / counts)
6. **Phase 2.1**: decide_prefetch_allowed ユニットテスト (= boundary 含む 7-9 ケース)
7. **analyze_perf.py scroll コマンド拡張**: 抑制イベントの併記
8. **Phase 2.2**: 手動 perf 検証 (= bookscan で再現、first_ready latency 計測)
