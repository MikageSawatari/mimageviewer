# 先読みが全枠を占有し、現在ページが待たされる

着手前に [CLAUDE.md](../../CLAUDE.md) の「バグ修正の一般原則」と
[docs/async-architecture.md](../async-architecture.md) を読むこと。
先行する試み: [pdf-fullscreen-page-promote.md](pdf-fullscreen-page-promote.md) (**効果が無かった**)。

## 1. 前回の修正が効かなかった理由 (実測で確定)

現在ページを Normal → HighNormal へ昇格させる修正を入れた。**昇格は実際に動いた**
(perf `pdf/pool_promote_fullscreen` が 43 件、`promoted` も出ている) が、**体感は変わらなかった。**

dispatcher を読むと理由が明白 ([pdf_loader.rs:2255](../../src/pdf_loader.rs:2255) 付近):

```rust
if let Some(j) = q.critical.pop_front() { ... }        // Critical は無条件に取る

let max_n = if reservation { worker_count - 1 } else { worker_count };
if q.normal_in_flight < max_n {
    if let Some(j) = q.high_normal.pop_front() { ... }  // ← Normal と同じ枠を共有
    if let Some(j) = q.normal.pop_front() { ... }
}
```

**HighNormal と Normal は同じ在庫枠 `max_n` を共有している。**ワーカー 5・予約 1 で `max_n = 4`。
先読みが 4 枠を埋めた時点で **HighNormal も開始できない**。昇格は**並び順を変えただけで、
ワーカーを 1 つも確保していなかった**。

## 2. 実測 (2026-08-20)

- **18 秒間ページが変わらなかった。**フレームは途切れていない (最大 0.49 秒) = アプリは正常。
- その 18 秒に走っていた `fs/load_begin` は **#3, #5〜#13 の 10 件、すべて `Normal` = 先読み**。
  同時に開始 (t=11.7) し、**それぞれ 18 秒**かかった。
- **対象ページ #1 自身は pending ですらなく**、キューで順番待ちしていた。

## 3. 直す方向 — 枠を分ける

### 案 B (推奨): Normal の上限を HighNormal より低くする

`max_n` を 1 つの値で共有するのをやめ、**Normal だけに低い上限**を与える。

- Critical: 従来どおり無条件 (予約枠の意味論を変えない)
- HighNormal: `worker_count - 1` まで
- Normal: **`worker_count - 2` まで** (最低 1 は確保して deadlock を防ぐ)

ワーカー 5 なら Critical 予約 1・Normal 上限 3・HighNormal 上限 4。**先読みが何件積まれても
HighNormal 用に最低 1 枠が空く。**既存の昇格機構 (動作確認済み) がそのまま活きる。

**利点**: cancel 不可のジョブが積み上がるリスクが無い。一覧の可視サムネ (同じく HighNormal) も
同時に救われる。Critical の予約意味論に触れない。

### 案 A (代替): 現在ページを Critical へ昇格し、外れたら降格

Critical は `max_n` の制約を受けないので確実に枠を取る。ただし**素早いページ送りで cancel 不可の
Critical が積み上がる**懸念がある (前回 Codex が HighNormal を選んだ理由)。降格でキュー上の
積み上がりは防げるが、**既に in-flight のものは降格できない**。

### 判断してほしいこと

**どちらを採るか、理由付きで決めて報告すること。**案 B が推奨だが、`max_n` の分割によって
一覧のサムネ生成が目に見えて遅くなるなら、それは別の退行である。**その懸念があるなら報告する。**

## 4. 制約

- **ワーカー数が少ない環境で deadlock させない。**現行コメントが警告しているとおり、
  1-worker pool で Normal が永久に動かなくなる形にしない。上限は必ず 1 以上に clamp する。
- 時間窓・delay・retry・一括 reset を使わない。**在庫枠は静的な設定から決まる**べきで、
  実行時の負荷で動的に変えない ([[feedback_deterministic_over_adaptive]] と同じ理由)。
- Critical の予約 (`CRITICAL_RESERVATION_ACTIVE`) の意味論を変えない。
- 前回入れた `promote_fullscreen_to_high_normal` と `pdf/pool_promote_fullscreen` は**残す**。
  案 B ならこれが初めて意味を持つ。

## 5. テスト

- Normal が上限に達しても **HighNormal が開始できる**こと (今回の実測ケース)。
- Critical が従来どおり無条件に取られること。
- worker_count = 1 / 2 で deadlock しないこと (Normal が永久に動かない状態を作らない)。
- 既存の dispatcher テストを弱体化させないこと。

## 6. 検証

実機で、**一覧の先読みが走っている最中に PDF を開いてページを送る**。
perf の `pdf/pool_queue_snapshot` で `normal` の in-flight が上限で頭打ちになり、
現在ページの `fs/load_begin` から提示までが短くなることを確認する。
